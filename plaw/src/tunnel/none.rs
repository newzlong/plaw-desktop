use super::Tunnel;
use anyhow::Result;

/// No-op tunnel — direct local access, no external exposure. Constructed
/// by `tunnel::create_tunnel` (gateway/mod.rs:595) for the default
/// tunnel-disabled config; the lib's dead-code pass doesn't trace
/// through the factory dispatch.
#[allow(dead_code)]
pub struct NoneTunnel;

#[async_trait::async_trait]
impl Tunnel for NoneTunnel {
    fn name(&self) -> &str {
        "none"
    }

    async fn start(&self, local_host: &str, local_port: u16) -> Result<String> {
        Ok(format!("http://{local_host}:{local_port}"))
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> bool {
        true
    }

    fn public_url(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_none() {
        let tunnel = NoneTunnel;
        assert_eq!(tunnel.name(), "none");
    }

    #[tokio::test]
    async fn start_returns_local_url() {
        let tunnel = NoneTunnel;
        let url = tunnel.start("127.0.0.1", 7788).await.unwrap();
        assert_eq!(url, "http://127.0.0.1:7788");
    }

    #[tokio::test]
    async fn stop_is_noop_success() {
        let tunnel = NoneTunnel;
        assert!(tunnel.stop().await.is_ok());
    }

    #[tokio::test]
    async fn health_check_is_always_true() {
        let tunnel = NoneTunnel;
        assert!(tunnel.health_check().await);
    }

    #[test]
    fn public_url_is_always_none() {
        let tunnel = NoneTunnel;
        assert!(tunnel.public_url().is_none());
    }
}
