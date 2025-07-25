use std::net::{IpAddr, SocketAddr};

use eyre::bail;
use tokio::net::TcpListener;

pub struct PortBinding {
    address: SocketAddr,
    listener: TcpListener,
}

impl PortBinding {
    pub async fn next_available(host: IpAddr, port: &mut u16) -> eyre::Result<PortBinding> {
        for _ in 0..100 {
            let address = (host, *port).into();

            let res = TcpListener::bind(address).await;

            *port += 1;

            if let Ok(listener) = res {
                return Ok(PortBinding { address, listener });
            }
        }

        bail!(
            "unable to select a port in range {}..={}",
            *port - 100,
            *port - 1
        );
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn into_socket_addr(self) -> SocketAddr {
        drop(self.listener);
        self.address
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::net::IpAddr;

    use super::*;

    #[tokio::test]
    async fn test_ports() -> eyre::Result<()> {

        let env_hosts = env::var("TEST_HOSTS").ok();

        let mut env_hosts = env_hosts
            .iter()
            .flat_map(|hosts| hosts.split(','))
            .map(|host| host.parse::<IpAddr>())
            .into_iter()
            .peekable();

        let default = env_hosts
            .peek()
            .map_or_else(|| Some(Ok([0, 0, 0, 0].into())), |_| None)
            .into_iter();

        let port = 2800;

        for host in default.chain(env_hosts) {
            let host = host?;
            test_port(host, port).await?;
        }

        Ok(())
    }

    async fn test_port(host: IpAddr, start_port: u16) -> eyre::Result<()> {
        let mut port = start_port;

        let bind1 = PortBinding::next_available(host, &mut port).await?;
        assert_eq!(port, bind1.port() + 1);

        let bind2 = PortBinding::next_available(host, &mut port).await?;
        assert_eq!(port, bind2.port() + 1);

        let port1 = bind1.into_socket_addr().port();
        let port2 = bind2.into_socket_addr().port();

        assert!(port1 < port2);

        let bind1 = PortBinding::next_available(host, &mut { port1 }).await?;
        let bind2 = PortBinding::next_available(host, &mut { port2 }).await?;

        assert_eq!(bind1.port(), port1);
        assert_eq!(bind2.port(), port2);

        Ok(())
    }
}
