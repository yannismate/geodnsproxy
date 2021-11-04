use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Condvar, Mutex};
use cidr_utils::cidr::{IpCidr, Ipv4Cidr};

struct GeoDnsProxyCfg {
  pub geo_zones: Vec<GeoZone>
}

struct GeoZone {
  pub name: String,
  pub cidr: IpCidr,
  pub nameserver: IpAddr
}

type ProxiedPacket = ([u8; 512], usize, SocketAddr);

fn main() {

  let z1 = GeoZone{
    name: String::from("local"),
    cidr: IpCidr::V4(Ipv4Cidr::from_str(String::from("127.0.0.0/8")).unwrap()),
    nameserver: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))
  };

  let cfg = GeoDnsProxyCfg{geo_zones: vec![z1]};
  let cfg = Arc::new(cfg);

  let in_socket = UdpSocket::bind("127.0.0.1:53").expect("Could not bind port");
  let out_socket = UdpSocket::bind("0.0.0.0:0").expect("Could not bind outgoing port");

  let in_socket_cl = in_socket.try_clone().expect("Could not clone main socket");
  let out_socket_cl = out_socket.try_clone().expect("Could not clone outgoing socket");

  let outgoing_queue = Arc::new((Mutex::new(VecDeque::<ProxiedPacket>::new()), Condvar::new()));
  let response_queue = Arc::new((Mutex::new(VecDeque::<ProxiedPacket>::new()), Condvar::new()));

  let outgoing_queue_cl = outgoing_queue.clone();
  let response_queue_cl = response_queue.clone();

  let id_addr_map = Arc::new(Mutex::new(HashMap::<u16, SocketAddr>::new()));
  let id_addr_map_cl = id_addr_map.clone();

  //Thread to write incoming socket
  std::thread::spawn(move || {
    let socket = in_socket_cl;
    loop {

      let mut packet = loop {
        let mut queue = response_queue_cl.0.lock().unwrap();
        match queue.pop_front() {
          Some(packet) => break packet,
          None => queue = response_queue_cl.1.wait(queue).unwrap()
        }
      };

      let out_buf = &mut packet.0[..packet.1];
      socket.send_to(out_buf, packet.2).unwrap_or_else(|err| {println!("[ERR] Responding DNS packet: {:?}", err); 0});

    }
  });

  //Thread to read from outgoing socket
  std::thread::spawn(move || {
    let socket = out_socket_cl;

    loop {
      let mut buf = [0; 512];
      let (amt, _src) = socket.recv_from(&mut buf).expect("");

      let packet_id = get_packet_id(&buf);
      let mut mapping = id_addr_map_cl.lock().unwrap();
      if mapping.contains_key(&packet_id) {
        let addr = mapping.remove(&packet_id).unwrap();
        response_queue.0.lock().unwrap().insert(0, (buf, amt, addr));
        response_queue.1.notify_one();
      } else {
        println!("[WARN] Could not find return address for id {}", &packet_id);
      }
    }
  });

  //Thread to write to outgoing socket
  std::thread::spawn(move || {
    let out_socket = out_socket;

    loop {

      let mut packet = loop {
        let mut queue = outgoing_queue.0.lock().unwrap();
        match queue.pop_front() {
          Some(packet) => break packet,
          None => queue = outgoing_queue.1.wait(queue).unwrap()
        }
      };
      let addr_ns = get_ns_addr(&cfg.geo_zones, packet.2.ip());
      match addr_ns {
        None => println!("[WARN] Could not find GeoZone for packet from {}", packet.2),
        Some(addr) => {
          let out_buf = &mut packet.0[..packet.1];
          let packet_id = get_packet_id(&out_buf);
          id_addr_map.lock().unwrap().insert(packet_id, packet.2);
          out_socket.send_to(out_buf, SocketAddr::new(addr, 53)).unwrap_or_else(|err| {println!("[ERR] Outgoing DNS packet: {:?}", err); 0});
        }
      }

    }
  });

  //Read from incoming socket
  loop {
    let mut buf = [0; 512];
    let (amt, src) = in_socket.recv_from(&mut buf).expect("");

    let mut queue = outgoing_queue_cl.0.lock().unwrap();
    queue.insert(0, (buf, amt, src));
    outgoing_queue_cl.1.notify_one();
  }

}

fn get_ns_addr(zones: &Vec<GeoZone>, addr: IpAddr) -> Option<IpAddr> {
  zones.iter()
    .filter(|z| {z.cidr.contains(addr)})
    .min_by_key(|z| {z.cidr.size()})
    .and_then(|z| {println!("[INFO] Packet from {} routed to zone {}", addr, z.name); Some(z.nameserver)})
}

#[inline]
fn get_packet_id(packet: &[u8]) -> u16 {
  ((packet[0] as u16) << 8) | packet[1] as u16
}
