
use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, str::FromStr, net::{SocketAddr, IpAddr, Ipv4Addr, Ipv6Addr}};
use anyhow::{Result, Context, bail};
use bytes::{BytesMut, BufMut, Buf};
use clap::Parser;
use tokio::{net::{TcpListener, TcpStream, tcp::{ReadHalf, WriteHalf}, UdpSocket}, io::{AsyncReadExt, AsyncWriteExt}};
use tracing::{info, error};
use url::Url;

mod args;
mod log;


fn main() -> Result<()> {
    log::init_with_filters("")?;

    tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()?
    .block_on(run_me())
}

async fn run_me() -> Result<()> { 
    let arg = args::Args::parse();
    
    let url1: AddrUrl = arg.leg1.parse().with_context(||"invalid leg1")?;
    let url2: AddrUrl = arg.leg2.parse().with_context(||"invalid leg2")?;

    if url1.url().scheme().eq_ignore_ascii_case("tcp-server")
    && url2.url().scheme().eq_ignore_ascii_case("tcp-client") {
        tcp_server_to_tcp_client(url1.into(), url2.into()).await?;

    } else if url1.url().scheme().eq_ignore_ascii_case("udp-server")
    && url2.url().scheme().eq_ignore_ascii_case("tcp-server") {
        udp_server_to_tcp_server(url1.into(), url2.into()).await?;

    } else {
        error!("Unsupported relay scenario");
    }
    
    Ok(())
}

async fn udp_server_to_tcp_server(url1: Arc<AddrUrl>, url2: Arc<AddrUrl>) -> Result<()> { 
    
    let udp_socket = UdpSocket::bind(url1.addr()).await
    .with_context(||format!("fail to bind at [{}]", url1.addr()))?
    .into_arc();

    let tcp_listener = TcpListener::bind(url2.addr()).await
    .with_context(||format!("fail to listen at [{}]", url2.url()))?;

    info!("Ucp service at [{}]", url1.addr());
    info!("Tcp listening at [{}]", url2.addr());

    let is_servicing = AtomicBool::new(false).into_arc();

    loop {
        let (socket, remote_addr) = tcp_listener.accept().await
        .with_context(||"accept tcp fail")?;

        if is_servicing.load(Ordering::Relaxed) { 
            info!("in servicing, ignore tcp from [{:?}]", remote_addr);
            continue;
        }

        is_servicing.store(true, Ordering::Relaxed);

        info!("tcp connected from [{:?}]", remote_addr);
        
        let udp_socket = udp_socket.clone();

        let is_servicing = is_servicing.clone();
        tokio::spawn(async move {
            let r = udp_tcp_task(udp_socket, socket).await;
            info!("disconnect reason [{:?}]", r);
            is_servicing.store(false, Ordering::Relaxed);
            r
        });
    }

}

async fn udp_tcp_task(udp1: Arc<UdpSocket>, mut tcp_socket: TcpStream) -> Result<()> {
    let (rd, wr) = tcp_socket.split();
    let udp2 = udp1.clone();

    tokio::select! {
        r = relay_tcp_to_udp(rd, udp2) => { r?; }
        r = relay_udp_to_tcp(udp1, wr) => { r?; }
    }

    Ok(())
}

async fn relay_tcp_to_udp(mut reader: ReadHalf<'_>, writer: Arc<UdpSocket>) -> Result<()> { 
    let mut buf = BytesMut::new();
    loop {
        let (packet_len, payload_len, addr) = read_tcp_packet(&mut reader, &mut buf).await?;

        let n = writer.send_to(&buf[..payload_len], addr).await?;
        if n!= packet_len {
            bail!("send_to but partial data sent")
        }

        buf.advance(packet_len);
    }
}

async fn read_tcp_packet(reader: &mut ReadHalf<'_>, buf: &mut BytesMut) -> Result<(usize, usize, SocketAddr)> {
    
    while buf.len() < 2 {
        let n = reader.read_buf(buf).await?;
        if n == 0 {
            bail!("read zero disconnected")
        }
    }

    let len = buf.get_u16() as usize;
    if len == 0 {
        bail!("zero tcp packet")
    }

    while buf.len() < len {
        let n = reader.read_buf(buf).await?;
        if n == 0 {
            bail!("read zero disconnected")
        }
    }

    let addr_len = buf[len-1] as usize;
    let addr = if addr_len > len {
        bail!("invalid addr len in tcp packet")

    } else if addr_len == 7 {
        let port = (&buf[len-7..len-5]).get_u16();
        let ip: [u8; 4] = (&buf[len-5..len-1]).try_into()?;
        let ip = Ipv4Addr::from(ip);
        SocketAddr::new(IpAddr::V4(ip), port) 

    } else if addr_len == 19 { 
        let port = (&buf[len-19..len-17]).get_u16();
        let ip: [u8; 16] = (&buf[len-17..len-1]).try_into()?;
        let ip = Ipv6Addr::from(ip);
        SocketAddr::new(IpAddr::V6(ip), port) 

    } else {
        bail!("unknown addr len in tcp packet")
    };

    Ok((len, len-addr_len, addr))
}


async fn relay_udp_to_tcp(reader: Arc<UdpSocket>, mut writer: WriteHalf<'_>) -> Result<()> { 
    let mut buf = vec![0_u8; 1800]; 
    loop { 
        let (data_len, addr) = reader.recv_from(&mut buf[2..]).await?;
        if data_len == 0 {
            bail!("udp recv zero bytes from [{}]", addr)
        }
        
        let addr_len = socket_addr_to_u8_slice_r(&addr, &mut buf[2+data_len..]);

        let len = data_len + addr_len;
        (&mut buf[0..2]).put_u16(len as u16);

        writer.write_all(&buf[..len]).await?;
    }
}

pub fn socket_addr_to_u8_slice_r(addr: &SocketAddr, data: &mut [u8]) -> usize { 
    
    (&mut data[0..2]).put_u16(addr.port());

    match &addr.ip() {
        IpAddr::V4(v) => { 
            let octets = v.octets();
            data[2..].copy_from_slice(&octets[..]);
            data[octets.len()] = octets.len() as u8;
            2 + octets.len() + 1
        },
        IpAddr::V6(v) => { 
            let octets = v.octets();
            data[2..].copy_from_slice(&octets[..]);
            data[octets.len()] = octets.len() as u8;
            2 + octets.len() + 1
        }
    }
}

pub struct SocketAddrBin;

impl SocketAddrBin {
    pub const LEN: usize = 7;
    
}


async fn tcp_server_to_tcp_client(url1: Arc<AddrUrl>, url2: Arc<AddrUrl>) -> Result<()> {

    let listener = TcpListener::bind(url1.addr()).await
    .with_context(||format!("fail to listen at [{}]", url1.url()))?;
    info!("Tcp listening at [{}]", url1.addr());


    loop {
        let (socket, remote_addr) = listener.accept().await
        .with_context(||"accept tcp fail")?;
        info!("tcp connected from [{:?}]", remote_addr);
        
        let url2 = url2.clone();
        tokio::spawn(async move {
            let r = tcp_only_task(socket, &url2).await;
            info!("disconnect reason [{:?}]", r);
            r
        });
    }
}

async fn tcp_only_task(mut socket1: TcpStream, url2: &Arc<AddrUrl>) -> Result<()> { 
    let mut socket2 = TcpStream::connect(url2.addr()).await
    .with_context(||format!("fail to tcp connect to [{}]", url2.url()))?;

    let (rd1, wr1) = socket1.split();

    let (rd2, wr2) = socket2.split();

    tokio::select! {
        r = relay_tcp_stream(rd1, wr2) => { r?; }, 
        r = relay_tcp_stream(rd2, wr1) => { r?; }, 
    }

    Ok(())
}

async fn relay_tcp_stream(mut reader: ReadHalf<'_>, mut writer: WriteHalf<'_>) -> Result<()> { 
    let mut buf = BytesMut::new();
    loop {
        let n = reader.read_buf(&mut buf).await?;
        if n == 0 {
            bail!("read zero disconnected")
        }

        writer.write_all_buf(&mut buf).await?;
    }
}



pub trait ToAddrString {
    fn to_addr_string(&self) -> Result<String>;
}

impl ToAddrString for Url {
    fn to_addr_string(&self) -> Result<String> {
        let host = self.host_str().with_context(||"no host")?;
        let port = self.port_or_known_default().with_context(||"no port")?;
        let addr = format!("{}:{}", host, port);
        Ok(addr)
    }
}

pub struct AddrUrl {
    url: Url,
    addr: String,
}

impl AddrUrl {
    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub fn url(&self) -> &Url {
        &self.url
    }
}

impl FromStr for AddrUrl {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url: Url = s.parse()?;
        let addr = url.to_addr_string()?;
        Ok(Self {
            url,
            addr,
        })
    }
}

pub trait IntoArc {
    fn into_arc(self) -> Arc<Self>;
}

impl<T> IntoArc for T {
    fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }
}


// pub struct TcpServerLeg {
//     listener: TcpListener,
// }

// impl TcpServerLeg {
//     pub async fn make(url: &Url) -> Result<Self> {
//         let addr = url.to_addr_string()?;
//         let listener = TcpListener::bind(&addr).await
//         .with_context(||format!("fail to listen at [{}]", url))?;

//         Ok(Self{listener})
//     }
// }



// trait ParseUrl {
//     fn parse_url(&self) -> Result<Url>;
// }

// impl ParseUrl for &str {
//     fn parse_url(&self) -> Result<Url> {
//         let mut url: Url = self.parse()?;
//         let port = url.port_or_known_default().with_context(||"no port")?;
//         url.set_port(Some(port));
//         Ok(url)
//     }
// }

// pub struct AddrUrl {
//     url: Url,
//     sock_addr: Option<SocketAddr>,
// }

// #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
// enum HostType { 
//     IpPrivate = 1, 
//     // IpGlobal,
//     IpOther,
//     IpLoopback,
//     Domain,
// }

// fn sort_addrs(addrs: &mut Vec<String>) -> Result<()> { 
//     let mut typed_addrs = Vec::with_capacity(addrs.len());
    
//     while let Some(s) = addrs.pop() {
//         let url = Url::parse(s.as_str())
//         .with_context(||format!("invalid url [{}]", s))?;

//         let host = url.host_str()
//         .with_context(||format!("no host of url [{}]", s))?;

//         let r = host.parse::<IpAddr>();
        
//         match r {
//             Ok(ip) => {
//                 if ip.is_unspecified() {
                    
//                 } else if ip.is_loopback() {
//                     typed_addrs.push((HostType::IpLoopback, s));

//                 } else if is_private_ip(&ip) {
//                     typed_addrs.push((HostType::IpPrivate, s));

//                 } 
//                 // else if ip.is_global() { // 要用到 use of unstable library feature 'ip'
//                 //     typed_addrs.push((HostType::IpGlobal, s));

//                 // } 
//                 else { 
//                     typed_addrs.push((HostType::IpOther, s));
//                 }
//             },
//             Err(_e) => {
//                 typed_addrs.push((HostType::Domain, s));
//             },
//         };
//     }

//     typed_addrs.sort_by(|x, y| x.0.cmp(&y.0) );

//     for v in typed_addrs {
//         addrs.push(v.1);
//     }

//     Ok(())
// }
