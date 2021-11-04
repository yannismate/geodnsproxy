use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;
use cidr_utils::cidr::{IpCidr, Ipv4Cidr};
use rayon::{ThreadPool, ThreadPoolBuilder};

struct AppState {
  pub cfg: GeoDnsProxyCfg,
  pub threads: ThreadPool
}

struct GeoDnsProxyCfg {
  pub geo_zones: Vec<GeoZone>
}

struct GeoZone {
  pub name: String,
  pub cidr: IpCidr,
  pub nameserver: IpAddr
}

fn main() {

  let z1 = GeoZone{
    name: String::from("local"),
    cidr: IpCidr::V4(Ipv4Cidr::from_str(String::from("127.0.0.0/8")).unwrap()),
    nameserver: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))
  };

  let cfg = GeoDnsProxyCfg{geo_zones: vec![z1]};
  //Automatically use 1 thread per logical cpu core
  let threads = ThreadPoolBuilder::new().build().expect("Could not start thread pool");

  let state = Arc::new(AppState {
    cfg,
    threads
  });

  let sock = UdpSocket::bind("127.0.0.1:53").expect("Could not bind port");

  loop {
    handle_incoming(&sock, &state).unwrap_or_else(|err| println!("Error while handling packet: {:?}", err));
  }

}


fn handle_incoming(sock: &UdpSocket, state: &AppState) -> Result<(), Box<dyn Error>> {
  let mut buf = [0; 512];
  let (amt, src) = sock.recv_from(&mut buf)?;

  //Reduce buffer size to actual content length
  let buf = &mut buf[..amt];

  let smallest_zone = state.cfg.geo_zones.iter()
    .filter(|z| {z.cidr.contains(src.ip())})
    .min_by_key(|z| {z.cidr.size()})
    .ok_or(NoMatchingZoneError{})?;

  println!("Request from {} is in GeoZone {}. Proxying to {}...", src.ip(), smallest_zone.name, smallest_zone.nameserver);

  //Automatically assign port
  let proxy_sock = UdpSocket::bind("0.0.0.0:0").unwrap();
  proxy_sock.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
  let sock_cl = sock.try_clone()?;
  let proxy_sock_cl = proxy_sock.try_clone()?;
  state.threads.spawn(move || {
    let mut rec_buf = [0; 512];
    while let Ok((amt_rec, _src_rec)) = proxy_sock_cl.recv_from(&mut rec_buf) {
      let rec_buf = &mut rec_buf[..amt_rec];
      sock_cl.send_to(rec_buf, src).expect("Could not forward response");
    }
  });
  proxy_sock.send_to(buf, SocketAddr::new(smallest_zone.nameserver, 53)).unwrap();

  Ok(())

}



struct NoMatchingZoneError;

impl Debug for NoMatchingZoneError {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_str("No matching zone!")
  }
}

impl Display for NoMatchingZoneError {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    f.write_str("No matching zone!")
  }
}

impl Error for NoMatchingZoneError {}