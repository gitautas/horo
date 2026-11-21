//! A sans-I/O TURN client (RFC 8656) for relaying one peer connection.
//!
//! Like str0m, it does no I/O. Bytes from the server go into
//! [`Turn::handle`], datagrams for a peer go through [`Turn::send_to`], and
//! everything for the server comes back as buffers to write. The caller owns
//! the transport: UDP, or a TCP/TLS stream framed with [`frame_len`].
//!
//! What it covers:
//! - long-term credentials: the 401 → REALM/NONCE → MESSAGE-INTEGRITY
//!   exchange, including a 438 stale-nonce retry;
//! - Allocate, CreatePermission, ChannelBind, Refresh; Send/Data indications
//!   until a channel is bound, ChannelData after;
//! - the three lifetimes: allocation (refreshed a minute before it lapses),
//!   permissions (5 min) and channel bindings (10 min, refreshed separately:
//!   missing that cut a call at exactly ten minutes in Phase 0);
//! - retransmission over UDP (RFC 8489 §6.2.1: 500 ms doubling, 7 tries),
//!   one timeout over TCP/TLS;
//! - IPv4 and IPv6 addresses.
//!
//! Not covered: FINGERPRINT, DONT-FRAGMENT, TCP allocations (RFC 6062, which
//! WebRTC doesn't use), ALTERNATE-SERVER.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::{Duration, Instant},
};

use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use sha1::Sha1;

const MAGIC: u32 = 0x2112_A442;

// Methods with the class bits folded in (all ours are below 0x10).
const ALLOCATE: u16 = 0x003;
const REFRESH: u16 = 0x004;
const SEND: u16 = 0x006;
const DATA: u16 = 0x007;
const CREATE_PERMISSION: u16 = 0x008;
const CHANNEL_BIND: u16 = 0x009;
const CLASS_MASK: u16 = 0x110;
const REQUEST: u16 = 0x000;
const INDICATION: u16 = 0x010;
const ERROR: u16 = 0x110;

const ATTR_USERNAME: u16 = 0x0006;
const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
const ATTR_ERROR_CODE: u16 = 0x0009;
const ATTR_CHANNEL_NUMBER: u16 = 0x000C;
const ATTR_LIFETIME: u16 = 0x000D;
const ATTR_XOR_PEER_ADDRESS: u16 = 0x0012;
const ATTR_DATA: u16 = 0x0013;
const ATTR_REALM: u16 = 0x0014;
const ATTR_NONCE: u16 = 0x0015;
const ATTR_XOR_RELAYED_ADDRESS: u16 = 0x0016;
const ATTR_REQUESTED_TRANSPORT: u16 = 0x0019;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// UDP: the only relay transport WebRTC uses.
const TRANSPORT_UDP: u8 = 17;
const DEFAULT_LIFETIME: Duration = Duration::from_secs(600);
/// Refresh the allocation this long before it lapses.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
/// Permissions last 5 minutes (RFC 8656 §9).
const PERMISSION_REFRESH: Duration = Duration::from_secs(240);
/// Channel bindings last 10 minutes (§12) and are not kept alive by traffic
/// or by the allocation's refresh.
const CHANNEL_REFRESH: Duration = Duration::from_secs(300);
/// RFC 8489 §6.2.1: initial RTO, and the number of transmissions.
const RTO: Duration = Duration::from_millis(500);
const MAX_TRANSMISSIONS: u32 = 7;
/// The last wait after the final transmission (Rm × RTO), and the whole
/// timeout over a reliable transport (§6.2.2).
const LAST_WAIT: Duration = Duration::from_millis(8000);
const RELIABLE_TIMEOUT: Duration = Duration::from_millis(39_500);

/// What came of a message from the server.
#[derive(Debug, PartialEq)]
pub enum TurnEvent {
    /// Nothing for the caller; replies, if any, are in [`Turn::outgoing`].
    None,
    /// The allocation exists. `mapped` is our address as the server sees it.
    Allocated { relayed: SocketAddr, mapped: Option<SocketAddr>, lifetime: Duration },
    /// A datagram a peer sent to our relayed address.
    Data { peer: SocketAddr, payload: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TurnError {
    /// The server refused the allocation, or a refresh of it.
    #[error("TURN server refused the {what}: {code} {reason}")]
    Refused { what: &'static str, code: u16, reason: String },
    /// No answer to the allocation, or to a refresh of it.
    #[error("TURN server didn't answer the {0}")]
    Timeout(&'static str),
    #[error("TURN allocation without a relayed address")]
    NoRelayedAddress,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Request {
    Allocate,
    Refresh,
    Permission(IpAddr),
    Bind(SocketAddr, u16),
}

impl Request {
    fn name(self) -> &'static str {
        match self {
            Request::Allocate => "allocation",
            Request::Refresh => "allocation refresh",
            Request::Permission(_) => "permission",
            Request::Bind(..) => "channel binding",
        }
    }
}

struct Outstanding {
    what: Request,
    bytes: Vec<u8>,
    transmissions: u32,
    rto: Duration,
    deadline: Instant,
}

pub struct Turn {
    username: String,
    password: String,
    /// Over UDP requests are retransmitted; over TCP/TLS they aren't.
    reliable: bool,
    realm: Option<String>,
    nonce: Option<Vec<u8>>,
    key: Option<[u8; 16]>,
    outstanding: HashMap<[u8; 12], Outstanding>,
    relayed: Option<SocketAddr>,
    /// When the allocation must next be refreshed by.
    refresh_at: Option<Instant>,
    permissions: HashMap<IpAddr, Instant>,
    /// Channel numbers, reserved on first send to a peer.
    channels: HashMap<SocketAddr, u16>,
    /// Bindings the server confirmed, with when they were last confirmed.
    bound: HashMap<u16, (SocketAddr, Instant)>,
    next_channel: u16,
    /// Messages for the server that `handle` produced along the way.
    pub outgoing: Vec<Vec<u8>>,
}

impl Turn {
    /// `reliable` is true over TCP or TLS, where requests aren't retransmitted.
    pub fn new(username: &str, password: &str, reliable: bool) -> Turn {
        Turn {
            username: username.into(),
            password: password.into(),
            reliable,
            realm: None,
            nonce: None,
            key: None,
            outstanding: HashMap::new(),
            relayed: None,
            refresh_at: None,
            permissions: HashMap::new(),
            channels: HashMap::new(),
            bound: HashMap::new(),
            next_channel: 0x4000,
            outgoing: Vec::new(),
        }
    }

    /// Our relayed address, once allocated.
    pub fn relayed(&self) -> Option<SocketAddr> {
        self.relayed
    }

    /// The opening Allocate. Unauthenticated on purpose: the server answers
    /// 401 with the realm and nonce the real one has to use.
    pub fn allocate(&mut self, now: Instant) -> Vec<u8> {
        self.request(Request::Allocate, now)
    }

    /// Release the allocation (a Refresh with lifetime 0). Fire and forget.
    pub fn deallocate(&mut self, now: Instant) -> Option<Vec<u8>> {
        self.relayed?;
        let tid = rand::random();
        let mut m = Message::new(REFRESH | REQUEST, tid);
        m.attr(ATTR_LIFETIME, &0u32.to_be_bytes());
        let _ = now;
        Some(self.sign(m))
    }

    /// When [`poll`](Self::poll) next has something to do.
    pub fn next_timeout(&self) -> Option<Instant> {
        let retransmit = self.outstanding.values().map(|o| o.deadline).min();
        let refresh = self.refresh_at;
        let permission = self.permissions.values().map(|at| *at + PERMISSION_REFRESH).min();
        let channel = self.bound.values().map(|(_, at)| *at + CHANNEL_REFRESH).min();
        [retransmit, refresh, permission, channel].into_iter().flatten().min()
    }

    /// Retransmit, refresh and rebind whatever is due. An error means the
    /// allocation is gone and the relay can't be used any more.
    pub fn poll(&mut self, now: Instant) -> Result<Vec<Vec<u8>>, TurnError> {
        let mut out = Vec::new();

        // Retransmissions and timeouts.
        let mut expired = Vec::new();
        for (tid, o) in self.outstanding.iter_mut() {
            if now < o.deadline {
                continue;
            }
            if !self.reliable && o.transmissions < MAX_TRANSMISSIONS {
                o.transmissions += 1;
                o.rto *= 2;
                o.deadline = now + if o.transmissions == MAX_TRANSMISSIONS { LAST_WAIT } else { o.rto };
                out.push(o.bytes.clone());
            } else {
                expired.push(*tid);
            }
        }
        for tid in expired {
            let Some(o) = self.outstanding.remove(&tid) else { continue };
            match o.what {
                Request::Allocate | Request::Refresh => return Err(TurnError::Timeout(o.what.name())),
                // Forget it, so the next send to that peer asks again.
                Request::Permission(ip) => {
                    self.permissions.remove(&ip);
                }
                Request::Bind(peer, channel) => {
                    self.channels.remove(&peer);
                    self.bound.remove(&channel);
                }
            }
        }

        if let Some(at) = self.refresh_at
            && now >= at
        {
            self.refresh_at = None; // until the refresh is answered
            out.push(self.request(Request::Refresh, now));
        }
        let stale: Vec<IpAddr> = self
            .permissions
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= PERMISSION_REFRESH)
            .map(|(ip, _)| *ip)
            .collect();
        for ip in stale {
            self.permissions.insert(ip, now);
            out.push(self.request(Request::Permission(ip), now));
        }
        let rebind: Vec<(u16, SocketAddr)> = self
            .bound
            .iter()
            .filter(|(_, (_, at))| now.duration_since(*at) >= CHANNEL_REFRESH)
            .map(|(channel, (peer, _))| (*channel, *peer))
            .collect();
        for (channel, peer) in rebind {
            if let Some(entry) = self.bound.get_mut(&channel) {
                entry.1 = now; // not again until this one is answered or lost
            }
            out.push(self.request(Request::Bind(peer, channel), now));
        }
        Ok(out)
    }

    /// Wrap a datagram for `peer`. The first send to a peer also asks for a
    /// permission and a channel; until the server confirms the channel the
    /// data goes as a Send indication, then as ChannelData (4 bytes of header
    /// instead of ~36).
    pub fn send_to(&mut self, peer: SocketAddr, payload: &[u8], now: Instant) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if let std::collections::hash_map::Entry::Vacant(e) = self.permissions.entry(peer.ip()) {
            e.insert(now);
            out.push(self.request(Request::Permission(peer.ip()), now));
        }
        let channel = match self.channels.get(&peer) {
            Some(&channel) => channel,
            None => {
                let channel = self.next_channel;
                // 0x4000..=0x4FFF; wrapping would reuse a peer's number.
                self.next_channel = if channel == 0x4FFF { 0x4000 } else { channel + 1 };
                self.channels.insert(peer, channel);
                out.push(self.request(Request::Bind(peer, channel), now));
                channel
            }
        };
        if self.bound.get(&channel).is_some_and(|(p, _)| *p == peer) {
            let mut msg = Vec::with_capacity(4 + payload.len() + 3);
            msg.extend_from_slice(&channel.to_be_bytes());
            msg.extend_from_slice(&(payload.len() as u16).to_be_bytes());
            msg.extend_from_slice(payload);
            // Padding is required over TCP, harmless over UDP.
            msg.resize(msg.len().next_multiple_of(4), 0);
            out.push(msg);
        } else {
            let mut m = Message::new(SEND | INDICATION, rand::random());
            m.xor_address(ATTR_XOR_PEER_ADDRESS, peer);
            m.attr(ATTR_DATA, payload);
            out.push(m.finish(None));
        }
        out
    }

    /// Take one complete message from the server.
    pub fn handle(&mut self, msg: &[u8], now: Instant) -> Result<TurnEvent, TurnError> {
        if msg.len() >= 4 && msg[0] & 0xC0 == 0x40 {
            let channel = u16::from_be_bytes([msg[0], msg[1]]);
            let len = u16::from_be_bytes([msg[2], msg[3]]) as usize;
            return Ok(match (self.bound.get(&channel), msg.get(4..4 + len)) {
                (Some((peer, _)), Some(payload)) => TurnEvent::Data { peer: *peer, payload: payload.to_vec() },
                _ => TurnEvent::None,
            });
        }
        let Some(parsed) = Parsed::new(msg) else { return Ok(TurnEvent::None) };
        let method = parsed.kind & !CLASS_MASK;
        let class = parsed.kind & CLASS_MASK;

        if method == DATA && class == INDICATION {
            return Ok(match (parsed.xor_address(ATTR_XOR_PEER_ADDRESS), parsed.attr(ATTR_DATA)) {
                (Some(peer), Some(payload)) => TurnEvent::Data { peer, payload: payload.to_vec() },
                _ => TurnEvent::None,
            });
        }

        let Some(outstanding) = self.outstanding.remove(&parsed.tid) else { return Ok(TurnEvent::None) };
        let what = outstanding.what;

        if class == ERROR {
            let error = parsed.attr(ATTR_ERROR_CODE).filter(|v| v.len() >= 4);
            let code = error.map(|v| (v[2] & 0x7) as u16 * 100 + v[3] as u16).unwrap_or(0);
            let reason = error.map(|v| String::from_utf8_lossy(&v[4..]).into_owned()).unwrap_or_default();
            if code == 401 || code == 438 {
                let first = self.key.is_none();
                if let Some(realm) = parsed.attr(ATTR_REALM) {
                    self.realm = Some(String::from_utf8_lossy(realm).into_owned());
                }
                if let Some(nonce) = parsed.attr(ATTR_NONCE) {
                    self.nonce = Some(nonce.to_vec());
                }
                if let Some(realm) = &self.realm {
                    let mut md5 = Md5::new();
                    md5.update(format!("{}:{realm}:{}", self.username, self.password));
                    self.key = Some(md5.finalize().into());
                }
                // A 401 after we already signed means the credentials are wrong.
                if first || code == 438 {
                    let retry = self.request(what, now);
                    self.outgoing.push(retry);
                    return Ok(TurnEvent::None);
                }
            }
            return match what {
                Request::Allocate | Request::Refresh => Err(TurnError::Refused { what: what.name(), code, reason }),
                Request::Permission(ip) => {
                    tracing::warn!("TURN refused a permission for {ip}: {code} {reason}");
                    Ok(TurnEvent::None)
                }
                Request::Bind(peer, channel) => {
                    // Fall back to Send indications for this peer.
                    tracing::warn!("TURN refused channel {channel:#06x} for {peer}: {code} {reason}");
                    Ok(TurnEvent::None)
                }
            };
        }

        Ok(match what {
            Request::Allocate => {
                let relayed = parsed.xor_address(ATTR_XOR_RELAYED_ADDRESS).ok_or(TurnError::NoRelayedAddress)?;
                let lifetime = parsed.lifetime().unwrap_or(DEFAULT_LIFETIME);
                self.relayed = Some(relayed);
                self.refresh_at = Some(now + lifetime.saturating_sub(REFRESH_MARGIN));
                TurnEvent::Allocated { relayed, mapped: parsed.xor_address(ATTR_XOR_MAPPED_ADDRESS), lifetime }
            }
            Request::Refresh => {
                let lifetime = parsed.lifetime().unwrap_or(DEFAULT_LIFETIME);
                self.refresh_at = Some(now + lifetime.saturating_sub(REFRESH_MARGIN));
                TurnEvent::None
            }
            Request::Permission(_) => TurnEvent::None,
            Request::Bind(peer, channel) => {
                self.bound.insert(channel, (peer, now));
                TurnEvent::None
            }
        })
    }

    fn request(&mut self, what: Request, now: Instant) -> Vec<u8> {
        let tid: [u8; 12] = rand::random();
        let method = match what {
            Request::Allocate => ALLOCATE,
            Request::Refresh => REFRESH,
            Request::Permission(_) => CREATE_PERMISSION,
            Request::Bind(..) => CHANNEL_BIND,
        };
        let mut m = Message::new(method | REQUEST, tid);
        match what {
            Request::Allocate => m.attr(ATTR_REQUESTED_TRANSPORT, &[TRANSPORT_UDP, 0, 0, 0]),
            Request::Refresh => m.attr(ATTR_LIFETIME, &(DEFAULT_LIFETIME.as_secs() as u32).to_be_bytes()),
            Request::Permission(ip) => m.xor_address(ATTR_XOR_PEER_ADDRESS, SocketAddr::new(ip, 0)),
            Request::Bind(peer, channel) => {
                let mut v = channel.to_be_bytes().to_vec();
                v.extend_from_slice(&[0, 0]);
                m.attr(ATTR_CHANNEL_NUMBER, &v);
                m.xor_address(ATTR_XOR_PEER_ADDRESS, peer);
            }
        }
        let bytes = self.sign(m);
        let first_wait = if self.reliable { RELIABLE_TIMEOUT } else { RTO };
        self.outstanding.insert(
            tid,
            Outstanding { what, bytes: bytes.clone(), transmissions: 1, rto: RTO, deadline: now + first_wait },
        );
        bytes
    }

    /// Add the long-term credentials, once we have them.
    fn sign(&self, mut m: Message) -> Vec<u8> {
        if let (Some(realm), Some(nonce)) = (&self.realm, &self.nonce) {
            m.attr(ATTR_USERNAME, self.username.as_bytes());
            m.attr(ATTR_REALM, realm.as_bytes());
            m.attr(ATTR_NONCE, nonce);
        }
        m.finish(self.key.as_ref())
    }
}

/// The length of the complete message at the start of a TCP stream, if the
/// buffer holds enough to tell: STUN is 20 bytes plus its length, ChannelData
/// 4 plus its length padded to 4 (RFC 8656 §12.5).
pub fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if buf[0] & 0xC0 == 0x40 { Some((4 + len).next_multiple_of(4)) } else { Some(20 + len) }
}

struct Message {
    buf: Vec<u8>,
    tid: [u8; 12],
}

impl Message {
    fn new(kind: u16, tid: [u8; 12]) -> Message {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(&kind.to_be_bytes());
        buf.extend_from_slice(&[0, 0]);
        buf.extend_from_slice(&MAGIC.to_be_bytes());
        buf.extend_from_slice(&tid);
        Message { buf, tid }
    }

    fn attr(&mut self, kind: u16, value: &[u8]) {
        self.buf.extend_from_slice(&kind.to_be_bytes());
        self.buf.extend_from_slice(&(value.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(value);
        self.buf.resize(self.buf.len().next_multiple_of(4), 0);
    }

    fn xor_address(&mut self, kind: u16, addr: SocketAddr) {
        let port = addr.port() ^ (MAGIC >> 16) as u16;
        let mut v = Vec::with_capacity(20);
        match addr.ip() {
            IpAddr::V4(ip) => {
                v.extend_from_slice(&[0, 0x01]);
                v.extend_from_slice(&port.to_be_bytes());
                v.extend_from_slice(&(u32::from(ip) ^ MAGIC).to_be_bytes());
            }
            IpAddr::V6(ip) => {
                v.extend_from_slice(&[0, 0x02]);
                v.extend_from_slice(&port.to_be_bytes());
                let key = xor_key(&self.tid);
                v.extend(ip.octets().iter().zip(key).map(|(a, k)| a ^ k));
            }
        }
        self.attr(kind, &v);
    }

    /// Set the length, and sign with MESSAGE-INTEGRITY if there is a key.
    fn finish(mut self, key: Option<&[u8; 16]>) -> Vec<u8> {
        if let Some(key) = key {
            // The length must already count the integrity attribute (24 bytes)
            // when the HMAC is taken over everything before it.
            let len = (self.buf.len() - 20 + 24) as u16;
            self.buf[2..4].copy_from_slice(&len.to_be_bytes());
            let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC takes any key length");
            mac.update(&self.buf);
            let digest = mac.finalize().into_bytes();
            self.attr(ATTR_MESSAGE_INTEGRITY, &digest);
        } else {
            let len = (self.buf.len() - 20) as u16;
            self.buf[2..4].copy_from_slice(&len.to_be_bytes());
        }
        self.buf
    }
}

/// The magic cookie followed by the transaction id: what IPv6 addresses are
/// XORed with.
fn xor_key(tid: &[u8; 12]) -> [u8; 16] {
    let mut key = [0u8; 16];
    key[..4].copy_from_slice(&MAGIC.to_be_bytes());
    key[4..].copy_from_slice(tid);
    key
}

struct Parsed<'a> {
    kind: u16,
    tid: [u8; 12],
    attrs: Vec<(u16, &'a [u8])>,
}

impl<'a> Parsed<'a> {
    fn new(msg: &'a [u8]) -> Option<Parsed<'a>> {
        if msg.len() < 20 || u32::from_be_bytes(msg[4..8].try_into().ok()?) != MAGIC {
            return None;
        }
        let len = u16::from_be_bytes([msg[2], msg[3]]) as usize;
        let body = msg.get(20..20 + len)?;
        let mut attrs = Vec::new();
        let mut i = 0;
        while i + 4 <= body.len() {
            let kind = u16::from_be_bytes([body[i], body[i + 1]]);
            let alen = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
            let value = body.get(i + 4..i + 4 + alen)?;
            attrs.push((kind, value));
            i += (4 + alen).next_multiple_of(4);
        }
        Some(Parsed { kind: u16::from_be_bytes([msg[0], msg[1]]), tid: msg[8..20].try_into().ok()?, attrs })
    }

    fn attr(&self, kind: u16) -> Option<&'a [u8]> {
        self.attrs.iter().find(|(k, _)| *k == kind).map(|(_, v)| *v)
    }

    fn lifetime(&self) -> Option<Duration> {
        let v = self.attr(ATTR_LIFETIME)?;
        Some(Duration::from_secs(u32::from_be_bytes(v.get(..4)?.try_into().ok()?) as u64))
    }

    fn xor_address(&self, kind: u16) -> Option<SocketAddr> {
        let v = self.attr(kind)?;
        if v.len() < 8 {
            return None;
        }
        let port = u16::from_be_bytes([v[2], v[3]]) ^ (MAGIC >> 16) as u16;
        let ip = match v[1] {
            0x01 => IpAddr::V4(Ipv4Addr::from(u32::from_be_bytes(v[4..8].try_into().ok()?) ^ MAGIC)),
            0x02 if v.len() >= 20 => {
                let key = xor_key(&self.tid);
                let mut octets = [0u8; 16];
                for (i, o) in octets.iter_mut().enumerate() {
                    *o = v[4 + i] ^ key[i];
                }
                IpAddr::V6(Ipv6Addr::from(octets))
            }
            _ => return None,
        };
        Some(SocketAddr::new(ip, port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server reply to `request`: success or error, with attributes.
    fn reply(request: &[u8], class: u16, attrs: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let parsed = Parsed::new(request).unwrap();
        let method = parsed.kind & !CLASS_MASK;
        let mut m = Message::new(method | class, parsed.tid);
        for (kind, value) in attrs {
            m.attr(*kind, value);
        }
        m.finish(None)
    }

    fn xor_attr(tid: [u8; 12], addr: SocketAddr) -> Vec<u8> {
        let mut m = Message::new(0, tid);
        m.xor_address(0, addr);
        m.buf[24..].to_vec()
    }

    fn error_code(code: u16, reason: &str) -> Vec<u8> {
        let mut v = vec![0, 0, (code / 100) as u8, (code % 100) as u8];
        v.extend_from_slice(reason.as_bytes());
        v
    }

    /// Allocate through the 401 exchange, as a real server makes us.
    fn allocated(reliable: bool, now: Instant) -> Turn {
        let mut turn = Turn::new("user", "pass", reliable);
        let first = turn.allocate(now);
        let challenge = reply(
            &first,
            ERROR,
            &[(ATTR_ERROR_CODE, error_code(401, "Unauthorized")), (ATTR_REALM, b"example.org".to_vec()), (ATTR_NONCE, b"n0nce".to_vec())],
        );
        assert_eq!(turn.handle(&challenge, now), Ok(TurnEvent::None));
        let signed = turn.outgoing.pop().expect("a signed retry");
        assert!(Parsed::new(&signed).unwrap().attr(ATTR_MESSAGE_INTEGRITY).is_some());
        let relayed: SocketAddr = "203.0.113.7:49152".parse().unwrap();
        let tid = Parsed::new(&signed).unwrap().tid;
        let ok = reply(&signed, 0x100, &[(ATTR_XOR_RELAYED_ADDRESS, xor_attr(tid, relayed)), (ATTR_LIFETIME, 600u32.to_be_bytes().to_vec())]);
        assert!(matches!(turn.handle(&ok, now), Ok(TurnEvent::Allocated { relayed: r, .. }) if r == relayed));
        turn
    }

    #[test]
    fn long_term_key_is_md5_of_user_realm_password() {
        // RFC 8489 §9.2.2; the expected value is `printf user:realm:pass | md5`.
        let mut md5 = Md5::new();
        md5.update("user:realm:pass");
        let key: [u8; 16] = md5.finalize().into();
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "8493fbc53ba582fb4c044c456bdc40eb");
    }

    #[test]
    fn xor_addresses_round_trip_for_both_families() {
        for addr in ["192.0.2.1:32853", "[2001:db8::1:2]:3478"] {
            let addr: SocketAddr = addr.parse().unwrap();
            let mut m = Message::new(SEND | INDICATION, rand::random());
            m.xor_address(ATTR_XOR_PEER_ADDRESS, addr);
            let bytes = m.finish(None);
            assert_eq!(Parsed::new(&bytes).unwrap().xor_address(ATTR_XOR_PEER_ADDRESS), Some(addr));
        }
    }

    #[test]
    fn allocation_goes_through_the_401_exchange() {
        let turn = allocated(false, Instant::now());
        assert_eq!(turn.relayed(), Some("203.0.113.7:49152".parse().unwrap()));
    }

    #[test]
    fn wrong_credentials_fail_instead_of_looping() {
        let now = Instant::now();
        let mut turn = allocated(false, now);
        // A refresh answered with 401 after we signed: credentials are bad.
        let refresh = turn.poll(now + Duration::from_secs(541)).unwrap();
        assert_eq!(refresh.len(), 1);
        let denied = reply(&refresh[0], ERROR, &[(ATTR_ERROR_CODE, error_code(401, "Unauthorized"))]);
        assert!(matches!(turn.handle(&denied, now), Err(TurnError::Refused { code: 401, .. })));
    }

    #[test]
    fn udp_requests_are_retransmitted_then_give_up() {
        let start = Instant::now();
        let mut turn = Turn::new("u", "p", false);
        let first = turn.allocate(start);
        let mut now;
        let mut sent = 1;
        loop {
            now = turn.next_timeout().expect("a pending deadline");
            match turn.poll(now) {
                Ok(again) => {
                    for msg in &again {
                        assert_eq!(msg, &first, "a retransmission is the same request");
                    }
                    sent += again.len();
                }
                Err(e) => {
                    assert_eq!(e, TurnError::Timeout("allocation"));
                    break;
                }
            }
        }
        assert_eq!(sent, MAX_TRANSMISSIONS as usize);
        // RFC 8489's example: 0, 0.5, 1.5, 3.5, 7.5, 15.5, 31.5 s, giving up at 39.5 s.
        assert_eq!(now.duration_since(start), Duration::from_millis(39_500));
    }

    #[test]
    fn reliable_transports_wait_once() {
        let start = Instant::now();
        let mut turn = Turn::new("u", "p", true);
        turn.allocate(start);
        assert_eq!(turn.next_timeout(), Some(start + RELIABLE_TIMEOUT));
        assert_eq!(turn.poll(start + RELIABLE_TIMEOUT), Err(TurnError::Timeout("allocation")));
    }

    #[test]
    fn data_uses_channel_data_once_bound_and_channels_get_rebound() {
        let now = Instant::now();
        let mut turn = allocated(false, now);
        let peer: SocketAddr = "198.51.100.9:7882".parse().unwrap();

        // First send: permission, binding, and the data as a Send indication.
        let first = turn.send_to(peer, b"hello", now);
        assert_eq!(first.len(), 3);
        let kinds: Vec<u16> = first.iter().map(|m| u16::from_be_bytes([m[0], m[1]])).collect();
        assert_eq!(kinds, [CREATE_PERMISSION, CHANNEL_BIND, SEND | INDICATION]);

        // The server confirms both.
        for request in &first[..2] {
            assert_eq!(turn.handle(&reply(request, 0x100, &[]), now), Ok(TurnEvent::None));
        }

        // Now ChannelData: 4-byte header, padded to 4.
        let data = turn.send_to(peer, b"hello", now);
        assert_eq!(data, vec![vec![0x40, 0x00, 0x00, 0x05, b'h', b'e', b'l', b'l', b'o', 0, 0, 0]]);
        assert_eq!(frame_len(&data[0]), Some(12));
        assert_eq!(
            turn.handle(&data[0], now),
            Ok(TurnEvent::Data { peer, payload: b"hello".to_vec() }),
            "ChannelData from the server maps back to the bound peer",
        );

        // Five minutes on, the permission and the binding are refreshed —
        // the binding on its own, since nothing else keeps it alive.
        let later = turn.poll(now + CHANNEL_REFRESH).unwrap();
        let kinds: Vec<u16> = later.iter().map(|m| u16::from_be_bytes([m[0], m[1]])).collect();
        assert!(kinds.contains(&CHANNEL_BIND), "{kinds:x?}");
        assert!(kinds.contains(&CREATE_PERMISSION), "{kinds:x?}");
    }

    #[test]
    fn a_lost_binding_is_asked_for_again() {
        let now = Instant::now();
        let mut turn = allocated(false, now);
        let peer: SocketAddr = "198.51.100.9:7882".parse().unwrap();
        turn.send_to(peer, b"x", now);
        // Nothing answers: let every request run out.
        let mut t = now;
        while let Some(next) = turn.next_timeout() {
            if next > now + Duration::from_secs(60) {
                break;
            }
            t = next;
            turn.poll(t).unwrap();
        }
        let again = turn.send_to(peer, b"x", t);
        let kinds: Vec<u16> = again.iter().map(|m| u16::from_be_bytes([m[0], m[1]])).collect();
        assert_eq!(kinds, [CREATE_PERMISSION, CHANNEL_BIND, SEND | INDICATION]);
    }
}
