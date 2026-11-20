//! horo's call engine from a terminal: join a MatrixRTC call (or a bare
//! LiveKit room) and talk, with no UI.
//!
//! ```sh
//! horo-call login https://matrix.example.org alice   # asks for the password
//! horo-call join '#room:example.org'                  # or a !room:id
//! horo-call livekit wss://sfu.example.org <jwt>       # no Matrix
//! horo-call devices
//! horo-call logout
//! ```
//!
//! `join` and `livekit` take `--input <id>` and `--output <id>` (ids from
//! `horo-call devices`), `--no-echo-cancellation` (headphones only),
//! `--gate <dB>`, `--muted` and `--seconds <n>`. While in
//! a call, type `m` and Enter to toggle mute, `q` and Enter (or Ctrl-C) to
//! leave.
//!
//! The session lives in `$HORO_CALL_DATA` (default
//! `~/.local/share/horo-call`), secrets included, in `0600` files rather
//! than the keychain: this is a development tool, signed in as its own
//! Matrix device.
//!
//! Calls aren't end-to-end encrypted yet, like the WebView app's: an
//! Element Call room with encryption on hears nothing from us, and we hear
//! noise from it.

use std::{
    io::BufRead,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use horo_call::{
    Call, CallEvent, CallOptions, Joined,
    devices::{self, AudioDevices},
};
use horo_core::{AppState, Event, EventSink, MatrixCore, matrix::auth};
use matrix_sdk::ruma::{OwnedRoomAliasId, OwnedRoomId, RoomOrAliasId};

/// Re-publish the MatrixRTC membership well inside its four-hour lifetime.
const MEMBERSHIP_REFRESH: Duration = Duration::from_secs(60 * 60);

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// horo-core's events are for a UI; there is none.
struct NoUi;

impl EventSink for NoUi {
    fn emit(&self, _event: Event) {}
    fn open_url(&self, url: &str) -> Result<(), String> {
        eprintln!("open {url}");
        Ok(())
    }
}

fn usage() -> ! {
    eprintln!(
        "usage:
  horo-call login <homeserver> <username>
  horo-call join <#alias:server | !room:server> [options]
  horo-call livekit <url> <token> [options]
    options: --input <id>  --output <id>  --no-echo-cancellation
             --gate <dB>  --muted  --seconds <n>
  horo-call devices
  horo-call logout"
    );
    std::process::exit(2)
}

fn data_dir() -> PathBuf {
    std::env::var_os("HORO_CALL_DATA").map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("HOME").expect("HOME is not set");
        PathBuf::from(home).join(".local/share/horo-call")
    })
}

/// The flags `join` and `livekit` share.
struct CallFlags {
    devices: AudioDevices,
    gate: f32,
    muted: bool,
    seconds: Option<f64>,
}

fn call_flags(args: &[String]) -> CallFlags {
    let mut flags = CallFlags { devices: AudioDevices::default(), gate: horo_media::audio::gate::GATE_OFF_DB, muted: false, seconds: None };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-echo-cancellation" => flags.devices.cancel_echo = false,
            "--input" => flags.devices.input = Some(args.next().cloned().unwrap_or_else(|| usage())),
            "--output" => flags.devices.output = Some(args.next().cloned().unwrap_or_else(|| usage())),
            "--muted" => flags.muted = true,
            "--gate" => flags.gate = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--seconds" => flags.seconds = Some(args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage())),
            _ => usage(),
        }
    }
    flags
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,horo_call=info".into()))
        .with_writer(std::io::stderr)
        .init();
    horo_core::matrix::session::use_file_storage();
    // Before any TLS: with both aws-lc-rs and ring linked (matrix-sdk brings
    // ring), rustls panics rather than pick one.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("login") if args.len() == 3 => login(&args[1], &args[2]).await,
        Some("logout") => logout().await,
        Some("devices") => list_devices(),
        Some("join") if args.len() >= 2 => join(&args[1], call_flags(&args[2..])).await,
        Some("livekit") if args.len() >= 3 => talk(&args[1], &args[2], call_flags(&args[3..])).await,
        _ => usage(),
    };
    if let Err(e) = result {
        eprintln!("horo-call: {e}");
        std::process::exit(1);
    }
}

async fn login(homeserver: &str, username: &str) -> Fallible {
    let password = rpassword::prompt_password(format!("password for {username}: "))?;
    let state = AppState::default();
    let info = auth::login_password(Arc::new(NoUi), &state, &data_dir(), homeserver, username, &password).await?;
    eprintln!("signed in as {} (device {})", info.user_id, info.device_id);
    if let Some(core) = state.take().await {
        core.shutdown().await;
    }
    Ok(())
}

async fn logout() -> Fallible {
    let state = AppState::default();
    if auth::restore(Arc::new(NoUi), &state, &data_dir()).await?.is_none() {
        eprintln!("not signed in");
        return Ok(());
    }
    auth::logout(&state, &data_dir(), true).await?;
    eprintln!("signed out");
    Ok(())
}

fn list_devices() -> Fallible {
    for (kind, list) in [("microphones", horo_devices::inputs()?), ("speakers", horo_devices::outputs()?)] {
        println!("{kind}:");
        for d in list {
            println!("  {}{}  ({})", if d.is_default { "* " } else { "  " }, d.name, d.id);
        }
    }
    Ok(())
}

async fn restore() -> Fallible<(AppState, Arc<MatrixCore>)> {
    let state = AppState::default();
    let info = auth::restore(Arc::new(NoUi), &state, &data_dir())
        .await?
        .ok_or("not signed in; run `horo-call login <homeserver> <username>` first")?;
    eprintln!("signed in as {}", info.user_id);
    let core = state.core().await?;
    Ok((state, core))
}

/// The room's id, once the sync has brought it in.
async fn find_room(core: &MatrixCore, room: &str) -> Fallible<OwnedRoomId> {
    let room = <&RoomOrAliasId>::try_from(room).map_err(|e| format!("{room:?} is not a room id or alias: {e}"))?;
    let room_id = match OwnedRoomId::try_from(room.as_str()) {
        Ok(id) => id,
        Err(_) => {
            let alias = OwnedRoomAliasId::try_from(room.as_str())?;
            core.client.resolve_room_alias(&alias).await?.room_id
        }
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    while core.client.get_room(&room_id).is_none() {
        if Instant::now() > deadline {
            return Err(format!("{room_id} didn't arrive in a minute of syncing; are you in it?").into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(room_id)
}

async fn join(room: &str, flags: CallFlags) -> Fallible {
    let (state, core) = restore().await?;
    let room_id = find_room(&core, room).await?;
    let credentials = horo_rtc::join(&core, &room_id, false).await?;
    eprintln!("joined the call in {room_id} on {}{}", credentials.livekit_url, if credentials.ringing { ", ringing" } else { "" });

    let refresher = {
        let (core, room_id) = (core.clone(), room_id.clone());
        tokio::spawn(async move {
            let mut every = tokio::time::interval(MEMBERSHIP_REFRESH);
            every.tick().await;
            loop {
                every.tick().await;
                if let Err(e) = horo_rtc::refresh(&core, &room_id).await {
                    tracing::warn!("refreshing the call membership: {e}");
                }
            }
        })
    };

    let result = talk(&credentials.livekit_url, &credentials.jwt, flags).await;
    refresher.abort();
    if let Err(e) = horo_rtc::leave(&core, &room_id).await {
        eprintln!("withdrawing the call membership: {e}");
    }
    if let Some(core) = state.take().await {
        core.shutdown().await;
    }
    result
}

/// Join the LiveKit room, run the devices, and print what happens until the
/// call ends, the time is up, or the user leaves.
async fn talk(url: &str, token: &str, flags: CallFlags) -> Fallible {
    let options = CallOptions { gate_threshold: flags.gate, muted: flags.muted, ..Default::default() };
    let Joined { call, microphone, speaker, mut events } = Call::connect(url, token, options).await?;
    let audio = devices::start(microphone, speaker, flags.devices)?;
    eprintln!(
        "in the room as {}{}; m⏎ toggles mute, q⏎ leaves",
        call.identity,
        if audio.cancels_echo() { "" } else { " — no echo cancellation, use headphones" }
    );
    if flags.muted {
        eprintln!("muted");
    }

    // Lines from the terminal, read off the runtime.
    let (keys_tx, mut keys) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if keys_tx.send(line.trim().to_owned()).is_err() {
                return;
            }
        }
        // End of input (a pipe, /dev/null) isn't a request to leave:
        // --seconds or Ctrl-C ends those runs.
    });

    let deadline = flags.seconds.map(|s| tokio::time::Instant::now() + Duration::from_secs_f64(s));
    let until_deadline = async {
        match deadline {
            Some(at) => tokio::time::sleep_until(at).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(until_deadline);
    let mut speaking: Vec<String> = Vec::new();
    let mut loudest = horo_media::audio::gate::FLOOR_DB;
    let mut meter = tokio::time::interval(Duration::from_secs(1));

    let ended = loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(CallEvent::Connected) => eprintln!("connected"),
                Some(CallEvent::Participants(people)) => {
                    let names: Vec<&str> = people.iter().map(|p| p.identity.as_str()).collect();
                    eprintln!("in the call: {}", if names.is_empty() { "nobody else".to_owned() } else { names.join(", ") });
                }
                Some(CallEvent::Speaking(now)) if now != speaking => {
                    if !now.is_empty() {
                        eprintln!("speaking: {}", now.join(", "));
                    }
                    speaking = now;
                }
                Some(CallEvent::Level(reading)) => loudest = loudest.max(reading.level),
                Some(CallEvent::Ended(reason)) => break Some(reason),
                Some(_) => {}
                None => break Some("the engine stopped".into()),
            },
            _ = meter.tick() => {
                if std::env::var_os("HORO_CALL_METER").is_some() {
                    let echo = audio.echo_stats().map(|s| format!(
                        ", echo removed {}, delay {}",
                        s.erle_db.map_or("-".into(), |e| format!("{e:.1} dB")),
                        s.delay.map_or("-".into(), |d| format!("{d:?}")),
                    ));
                    eprintln!("mic {loudest:>6.1} dBFS{}{}", if call.is_muted() { " (muted)" } else { "" }, echo.unwrap_or_default());
                }
                loudest = horo_media::audio::gate::FLOOR_DB;
            }
            Some(key) = keys.recv() => match key.as_str() {
                "m" => {
                    call.set_muted(!call.is_muted());
                    eprintln!("{}", if call.is_muted() { "muted" } else { "unmuted" });
                }
                "q" => break None,
                _ => eprintln!("m⏎ toggles mute, q⏎ leaves"),
            },
            _ = &mut until_deadline => break None,
            _ = tokio::signal::ctrl_c() => break None,
        }
    };
    drop(audio);
    match ended {
        Some(reason) => Err(format!("call ended: {reason}").into()),
        None => {
            call.leave().await;
            eprintln!("left");
            Ok(())
        }
    }
}
