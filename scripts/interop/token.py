"""A LiveKit access token for the dev server (key devkey, secret secret).

python3 -I token.py <identity> <room> [secret]
"""
import base64, hashlib, hmac, json, sys, time

identity, room = sys.argv[1], sys.argv[2]
secret = sys.argv[3] if len(sys.argv) > 3 else "secret"
b64 = lambda data: base64.urlsafe_b64encode(data).rstrip(b"=").decode()
now = int(time.time())
header = b64(json.dumps({"alg": "HS256", "typ": "JWT"}).encode())
claims = b64(json.dumps({
    "iss": "devkey", "sub": identity, "nbf": now - 10, "exp": now + 3600,
    "video": {"room": room, "roomJoin": True, "canPublish": True, "canSubscribe": True},
}).encode())
signature = b64(hmac.new(secret.encode(), f"{header}.{claims}".encode(), hashlib.sha256).digest())
print(f"{header}.{claims}.{signature}")
