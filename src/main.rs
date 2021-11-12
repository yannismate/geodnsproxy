use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::str::FromStr;
use std::sync::{Arc, Condvar, Mutex};
use cidr_utils::cidr::IpCidr;

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

  let cfg = load_cfg();
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
          None => match response_queue_cl.1.wait(queue).unwrap().pop_front() {
            None => continue,
            Some(packet) => break packet
          }
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
          None => match outgoing_queue.1.wait(queue).unwrap().pop_front() {
            None => continue,
            Some(packet) => break packet
          }
        }
      };
      let addr_ns = get_ns_addr(&cfg.geo_zones, packet.2.ip());
      match addr_ns {
        None => println!("[WARN] Could not find GeoZone for packet from {}", packet.2),
        Some(addr) => {
          let out_buf = &mut packet.0[..packet.1];
          let packet_id = get_packet_id(out_buf);
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

const CFG_MALFORMED : &str = "config.json is malformed";
fn load_cfg() -> GeoDnsProxyCfg {
  let file = File::open("config.json").expect("Could not open config.json");
  let json : serde_json::Value = serde_json::from_reader(file).expect("config.json is not parseable");

  let zones : &Vec<serde_json::Value> = json.get("geo_zones").expect(CFG_MALFORMED)
    .as_array().expect(CFG_MALFORMED);

  let mut geo_zones = Vec::new();

  for zone in zones {
    let name = zone.get("name").expect(CFG_MALFORMED)
      .as_str().expect(CFG_MALFORMED).to_string();

    let cidr = IpCidr::from_str(
      zone.get("cidr").expect(CFG_MALFORMED).as_str().expect(CFG_MALFORMED)
    ).expect(&*format!("CIDR in zone {} is not valid", name));

    let nameserver = IpAddr::from_str(
      zone.get("nameserver").expect(CFG_MALFORMED).as_str().expect(CFG_MALFORMED)
    ).expect(&*format!("Nameserver address in zone {} is not valid", name));

    geo_zones.insert(0,GeoZone {
      name,
      cidr,
      nameserver
    });
  }

  GeoDnsProxyCfg {geo_zones}
}

fn get_ns_addr(zones: &[GeoZone], addr: IpAddr) -> Option<IpAddr> {
  zones.iter()
    .filter(|z| {z.cidr.contains(addr)})
    .min_by_key(|z| {z.cidr.size()})
    .map(|z| {println!("[INFO] Packet from {} routed to zone {}", addr, z.name); z.nameserver})
}

#[inline]
fn get_packet_id(packet: &[u8]) -> u16 {
  ((packet[0] as u16) << 8) | packet[1] as u16
}
