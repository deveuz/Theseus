//! This application pings a specific IPv4 address and gets ping statistics.
//! Important: QEMU does not support the ICMP protocol by default so it's important to 
//! run this command: sudo sh -c "echo \"0 2147483647\" > /proc/sys/net/ipv4/ping_group_range"
//! in the environment prior to running this application

#![no_std]
#![feature(slice_concat_ext)]

#[macro_use] extern crate log;
#[macro_use] extern crate alloc;
#[macro_use] extern crate terminal_print;
extern crate smoltcp;
extern crate network_manager;
extern crate byteorder;
extern crate hpet;
extern crate http_client;
extern crate hashbrown;
extern crate ota_update_client;
extern crate getopts;


use getopts::{Matches, Options};
use core::str::FromStr;
use hashbrown::HashMap;
use alloc::vec::Vec;        
use alloc::string::String;
use hpet::get_hpet;
use smoltcp::{
    socket::{SocketSet, IcmpSocket, IcmpSocketBuffer, IcmpPacketMetadata, IcmpEndpoint},
    wire::{IpAddress, Icmpv4Repr, Icmpv4Packet},
    phy::{ChecksumCapabilities},
};
use network_manager::{NetworkInterfaceRef, NETWORK_INTERFACES};
use byteorder::{ByteOrder, NetworkEndian};
use http_client::{millis_since, poll_iface};


macro_rules! hpet_ticks {
    () => {
        match get_hpet().as_ref().ok_or("coudln't get HPET timer") {
            Ok(time) => time.get_counter(),
            Err(_) => return println!("couldnt get HPET timer"),
        }; 
    };
}


#[no_mangle]
fn main(args: Vec<String>) -> isize {

    let mut opts = Options::new();
    opts.optflag("h", "help", "print this help menu");
    opts.optflag("v", "verbose", "a more detailed view of packets sent and received");
    opts.optopt("c", "count", "amount of echo request packets to send (default: 4)", "N");
    opts.optopt("i", "interval", "interval between packets being sent in miliseconds (default: 500)", "N");
    opts.optopt("t", "timeout", "maximum time between echo request and echo reply in milliseconds (default: 10000)", "N");
    
  
    let matches = match opts.parse(&args) {
        Ok(m) => m,
        Err(_f) => {
            println!("{}", _f);
            print_usage(&opts);
            return -1; 
        }
    };

    if matches.opt_present("h") {
        return print_usage(&opts);
        return 0;
    }

    let mut ping_address = IpAddress::Unspecified;

    if matches.free.len() != 0 {
        match IpAddress::from_str(&matches.free[0]) {
            Ok(address) => {
                ping_address = address;
                
            }
            _ => { 
                println!("Invalid argument {}, not a valid adress", matches.free[0]); 
                return -1;
            },
        };  
    
    }

    else {
        println!("no arguments provided");
        return 0;
    }
    
    let result = rmain(&matches, opts, ping_address);
    
    match result {
        Ok(_) => { 0 }
        Err(e) => {
            println!("Ping initialization failed: {}.", e);
            -1
        }
    }
}

pub fn rmain(matches: &Matches, _opts: Options, address: IpAddress) -> Result<(), &'static str> {

    
    let mut count = 4;
    let mut interval = 500;
    let mut timeout = 10000;
    let mut verbose = false;
    let did_work = true;


    if let Some(i) = matches.opt_default("c", "4") {
        count = i.parse::<usize>().map_err(|_e| "couldn't parse number of packets")?;
    }
    if let Some(i) = matches.opt_default("i", "500") {
        interval = i.parse::<i64>().map_err(|_e| "couldn't parse interval")?;
    }
    if let Some(i) = matches.opt_default("t", "10000") {
        timeout = i.parse::<i64>().map_err(|_e| "couldn't parse timeout length")?;
    }
    if matches.opt_present("v") {
        verbose = true;
    }
    
    if did_work {
        ping(address, count, interval, timeout, verbose);
        Ok(())
    }
    else {
        Ok(())
    }
}

/// Used to gain access to the ethernet interface
fn get_default_iface() -> Result<NetworkInterfaceRef, String> {
    NETWORK_INTERFACES.lock()
        .iter()
        .next()
        .cloned()
        .ok_or_else(|| format!("no network interfaces available"))
}

// Retrieves the echo reply contained in the recieve buffer and prints data pertaining to the packet
fn get_icmp_pong (waiting_queue: &mut HashMap<u16, i64>, times: &mut Vec<i64>, mut total_time: i64, 
    repr: Icmpv4Repr, mut recieved: u16, remote_addr: IpAddress, timestamp: i64)  {
    
    if let Icmpv4Repr::EchoReply { seq_no, data, ..} = repr {
        if let Some(_) = waiting_queue.get(&seq_no) {
            let packet_timestamp_ms = NetworkEndian::read_i64(data);
            
            println!("{} bytes from {}: icmp_seq={}, time={}ms",
                        data.len(), remote_addr, seq_no,
                        timestamp - packet_timestamp_ms);
            
            waiting_queue.remove(&seq_no);
            recieved += 1;
            times.push((timestamp - packet_timestamp_ms) as i64);
            total_time += timestamp - packet_timestamp_ms;
        }
    } 
}

fn ping(address: IpAddress, count: usize, interval: i64, timeout: i64, verbose: bool) {

    let startup_time = hpet_ticks!() as u64;
    let remote_addr = address;
    let mut times = Vec::new();
    
    // Initialize the ICMP sockett using the smoltcp function using a transmit packet buffer 
    // and receiving packet buffer
    //
    // The payload storage contains the application data and the transport header, and metadata contains the ICMP type
    // and ICMP code, which are used to classify between echo requests and echo replies
    let icmp_rx_buffer = IcmpSocketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; 256]);
    let icmp_tx_buffer = IcmpSocketBuffer::new(vec![IcmpPacketMetadata::EMPTY], vec![0; 256]);
    let icmp_socket = IcmpSocket::new(icmp_rx_buffer, icmp_tx_buffer);
    
    // Get the default ethernet interface to ping with
    let iface_result = get_default_iface();
    let iface = match iface_result {
        Ok(network) => network,
        Err(err) => return println!("couldn't initialize the network: {}", err),
    };

    let mut sockets = SocketSet::new(vec![]);
    let icmp_handle = sockets.add(icmp_socket);
    
    let mut send_at = match millis_since(startup_time as u64) {
        Ok(time) => time as i64,
        Err(err) => return println!("couldn't get time since start_up: {}", err),
    };
    
    let mut seq_no = 0;
    let received = 0;
    let total_time = 0;
    let mut echo_payload = [0xffu8; 40];

    // Designate no checksum capabilities 
    let checksum_caps = ChecksumCapabilities::ignored();
    
    // Initiate a hashmap
    let mut waiting_queue = HashMap::new();  
    
    // Portless icmp messages such as echo request require a 16-bit identifier to bind to
    // so that only icmp messages with this identifer can pass through the icmp socket
    let ident = 0x22b; 

    // Makes sure that the icmp handle can communicate with the given ethernet interface
    loop {
        
        match poll_iface(&iface, &mut sockets, startup_time) {
            Ok(_) => {},
            Err(e) => {
                debug!("poll error: {}", e);
            }
        }
        {
            let timestamp = match millis_since(startup_time as u64) {
                Ok(time) => time as i64,
                Err(err) => return println!("couldn't get timestamp:{}", err),
            };
            let mut socket = sockets.get::<IcmpSocket>(icmp_handle); 
            
            // Checks if the icmp socket is open, and only bind the identifier icmp to it if 
            // it is closed
            if !socket.is_open() {
                match socket.bind(IcmpEndpoint::Ident(ident)) {
                    Ok(_) => (),
                    Err(e) => return println!("the socket failed to bind: {}", e),
                }; 
                send_at = timestamp;
            }
            
            // Checks if the icmp sockett can send an echo request
            if socket.can_send() && seq_no < count as u16 && send_at <= timestamp {
                NetworkEndian::write_i64(&mut echo_payload, timestamp);
                let icmp_repr = Icmpv4Repr::EchoRequest{
                        ident: ident,
                        seq_no: seq_no,
                        data: &echo_payload
                    };
                let icmp_payload = match socket.send(icmp_repr.buffer_len(), remote_addr) {
                    Ok(payload) => payload,
                    Err(_err) => return println!("the icmp socket cannot send"),
                };
                let mut icmp_packet = Icmpv4Packet::new_unchecked(icmp_payload);
                
                
                icmp_repr.emit(&mut icmp_packet, &checksum_caps); //turns or "emits" the raw network stack into an icmpv4 packet,
                if verbose {
                    println!("buffer length: {}", icmp_repr.buffer_len());
                    println!("checking checksum of packet, should be 0: {:?}", icmp_packet.checksum());
                    println!("checking echo_ident of packet, should be a value: {:?}", icmp_packet.echo_ident());
                    println!("checking msg_type of packet, should be an echo_request: {:?}", icmp_packet.msg_type());
                }
            // Insert the sequence number into the waiting que along with the timestamp after an echo
            // Request has been sent
            waiting_queue.insert(seq_no, timestamp);
            seq_no += 1;
            send_at += interval;
            }

            // Once the socket can successfully recieve the echo reply, unload the payload and
            // then return the current time as well as wether the ping has been recieved         
            if socket.can_recv() {
                let (payload, _) = match socket.recv() {
                    Ok((packet_buff,end_point)) => (packet_buff, end_point),
                    Err(err) => return println!("err: {} the receive buffer is empty", err), 
                }; 
                let icmp_packet = match Icmpv4Packet::new_checked(&payload) {
                    Ok(packet) => packet,
                    Err(err) => return println!("err: {}", err),
                }; 
                // Turns or "parses" the ICMPv4 packet into a raw level representation
                let icmp_repr = match Icmpv4Repr::parse(&icmp_packet, &checksum_caps) {
                    Ok(repr) => repr,
                    Err(err) => return println!("err: {}", err),
                }; 
                
        
                get_icmp_pong(&mut waiting_queue, &mut times, total_time, icmp_repr, received, remote_addr, timestamp);
                if verbose {
                    println!("buffer length: {}", icmp_repr.buffer_len());
                    println!("checking checksum of packet, should be above 0: {:?}", icmp_packet.checksum());
                    println!("checking echo_ident of packet, should be a value: {:?}", icmp_packet.echo_ident());
                    println!("checking msg_type of packet, should be an echo_reply: {:?}", icmp_packet.msg_type());                
                }
            }
            
            // Uses this retain function to decide whether the sequence you're currently looking at is timed out 
            waiting_queue.retain(|seq, from| {
                if timestamp - *from <  timeout {
                    true
                } else {
                    println!("From {} icmp_seq={} timeout", remote_addr, seq);
                    false
                }
            });

            if seq_no == count as u16 && waiting_queue.is_empty()  {
                break
            }
        }
    

    }
    
    // Computes ping min/avg/max
    let avg_ping = if received != 0 {
        total_time as f64 / (received as f64)
    } else {
        0 as f64
    };
     
    let min_ping = match times.iter().min() {
            Some(min) => min,
            None => &(0 as i64),
    };

    let max_ping = match times.iter().max() {
            Some(max) => max,
            None => &(0 as i64),
    };
        
    
    
    
    println!("\n--- {} ping statistics ---", remote_addr);
    println!("{} packets transmitted, {} received, {:.0}% packet loss \nrtt min/avg/max = {}/{}/{}",
            seq_no, received, 100.0 * (seq_no - (received)) as f64 / seq_no as f64, min_ping, avg_ping as f64, max_ping);
    if received == 0{         
            println!("\nwarning: Ping/ICMP will not work in QEMU unless you specifically enable it. If you are able to ping  \nthe qemu gateway address 10.0.2.2 and not other addresses, your ICMP is most likely disabled");
    }


}

fn print_usage(opts: &Options) -> isize {
    let mut brief = format!("Usage: ping DESTINATION \n \n");

    brief.push_str("pings an IPv4 address and returns ping statistics");

    println!("{} \n", opts.usage(&brief));

    0
}