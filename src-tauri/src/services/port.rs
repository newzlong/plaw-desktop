use std::path::Path;

pub fn port_available(port: u16) -> bool {
    std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_ok()
}

pub fn load_saved_port(data_dir: &Path) -> Option<u16> {
    let path = data_dir.join("port-state.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("port").and_then(|v| v.as_u64()).map(|p| p as u16)
}

pub fn save_port(data_dir: &Path, port: u16) {
    let path = data_dir.join("port-state.json");
    let json = serde_json::json!({ "port": port });
    let _ = std::fs::write(&path, serde_json::to_string(&json).unwrap_or_default());
}

pub fn allocate_port(data_dir: &Path) -> u16 {
    if let Some(saved) = load_saved_port(data_dir) {
        if saved > 0 && port_available(saved) {
            return saved;
        }
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}
