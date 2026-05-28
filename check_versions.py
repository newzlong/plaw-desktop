import json, urllib.request, sys

crates = [
    # root workspace
    "statrs","tokio","tokio-stream","tokio-util","tokio-tungstenite","futures-util",
    "reqwest","serde","serde_json","toml","rusqlite","clap","clap_complete",
    "tracing","tracing-subscriber","anyhow","thiserror","sha2","async-trait",
    "indicatif","uuid","chrono",
    # tauri
    "tauri-build","tauri","tauri-plugin-dialog","tauri-plugin-fs",
    "tauri-plugin-notification","tauri-plugin-opener","tauri-plugin-process",
    "tauri-plugin-shell","log","dirs-next","chacha20poly1305","hex","flate2","tar",
    # plaw core (unique additions)
    "zip","matrix-sdk","serde_ignored","directories","shellexpand","schemars",
    "prometheus","base64","image","urlencoding","fast_html2md","nanohtml2text",
    "fantoccini","wasmi","hmac","rand","serde-big-array","parking_lot","ring",
    "prost","postgres","tokio-postgres-rustls","chrono-tz","iana-time-zone","cron",
    "dialoguer","rustyline","console","glob","which","tempfile","nostr-sdk","regex",
    "hostname","rustls","rustls-pki-types","tokio-rustls","webpki-roots","lettre",
    "mail-parser","async-imap","axum","tower","tower-http","http-body-util",
    "rust-embed","mime_guess","opentelemetry","opentelemetry_sdk","opentelemetry-otlp",
    "tokio-serial","winreg","nusb","probe-rs","pdf-extract","qrcode",
    "wa-rs","wa-rs-core","wa-rs-binary","wa-rs-proto","wa-rs-ureq-http","wa-rs-tokio-transport",
    "rppal","landlock","criterion","wiremock","scopeguard"
]

def latest(name):
    try:
        with urllib.request.urlopen(f"https://crates.io/api/v1/crates/{name}", timeout=15) as r:
            data = json.loads(r.read())
            return data["crate"]["newest_version"]
    except Exception as e:
        return f"err:{e}"

results = {}
for c in crates:
    results[c] = latest(c)
    print(f"{c} = {results[c]}")

with open("versions.json","w") as f:
    json.dump(results, f, indent=2)
