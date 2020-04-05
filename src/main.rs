use byteorder::{ByteOrder, NetworkEndian};
use clap::{App, Arg, SubCommand};
use std::io::{stdout, Write};
use std::net::UdpSocket;
use std::str::FromStr;
use std::thread;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

fn main() -> std::io::Result<()> {
    let args = App::new("Nettest")
        .version("0.1")
        .author("Jordan West <jordwest@gmail.com>")
        .about("Test the quality of a connection. Run without args to start a receiving server, then start a client elsewhere to start sending data. The server listens on UDP port 37131, ensure your firewall/NAT rules allow this port.")
        .subcommand(
            SubCommand::with_name("client")
                .about("Runs a client that sends messages to a nettest server somewhere")
                .arg(
                    Arg::with_name("destination")
                        .help("Address of the server to send to")
                        .required(true)
                        .index(1),
                )
                .arg(
                    Arg::with_name("rate")
                        .help("Number of messages to send per second")
                        .required(false),
                ),
        )
        .get_matches();

    if let Some(args) = args.subcommand_matches("client") {
        let addr = args.value_of("destination").unwrap();
        let rate = FromStr::from_str(args.value_of("rate").unwrap_or("10")).unwrap();
        client(addr, rate)
    } else {
        server()
    }
}

const PACKET_SIZE: usize = 1024;

fn client(addr: &str, rate: f32) -> std::io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:37132")?;
    let mut time = Instant::now();
    let mut to_send: f32 = 0.0;
    println!("Sending {}/sec to {}", rate, addr);

    let boot_time = Instant::now();

    let mut data = [0; PACKET_SIZE];
    let mut seq = 0;

    let s2 = socket.try_clone().expect("Could not clone the socket");
    thread::spawn(move || {
        client_reporter(s2, rate);
    });

    loop {
        // Send one packet for each millisecond passed
        let time_passed = Instant::now() - time;
        time = Instant::now();

        to_send += time_passed.as_secs_f32() * rate;

        let to_send_this_tick = to_send as u32;

        for _ in 0..to_send_this_tick {
            let packet = ClientPacket {
                time: (Instant::now() - boot_time).as_millis() as u64,
                seq,
                send_rate: rate,
            };
            seq += 1;
            packet.write(&mut data);
            socket
                .send_to(&data, &format!("{}:37131", addr))
                .expect("couldn't send data");
        }

        to_send = to_send - (to_send_this_tick as f32);

        sleep(Duration::from_millis(10));
    }
}

fn progress_bar(writer: &mut impl Write, value: f32) {
    use termion::color;

    let pct = match value {
        val if val < 0.0 => 0 as u32,
        val => (val * 100.0) as u32,
    };

    match pct {
        pct if pct < 70 => write!(writer, "{}", color::Bg(color::Red)).unwrap(),
        pct if pct < 97 => write!(writer, "{}", color::Bg(color::Yellow)).unwrap(),
        _ => write!(writer, "{}", color::Bg(color::Green)).unwrap(),
    };
    for v in 0..10 {
        write!(writer, " ").unwrap();

        if v * 10 > pct {
            write!(writer, "{}", color::Bg(color::Black)).unwrap();
        }
    }

    write!(writer, "{} {}%", termion::style::Reset, pct).unwrap();
}

fn client_reporter(socket: UdpSocket, rate: f32) {
    //let mut last_seq = 0;
    use termion::screen::AlternateScreen;
    let mut buf = [0; 8192];
    let mut remote_seq = 0;
    let mut screen = AlternateScreen::from(stdout());
    loop {
        socket.recv(&mut buf).unwrap();
        let reply = ServerPacket::read(&buf);
        if reply.seq > remote_seq {
            remote_seq = reply.seq;
            write!(
                screen,
                "{}{}",
                termion::clear::All,
                termion::cursor::Goto(1, 1)
            )
            .unwrap();
            write!(screen, "Rate\t\t\tData\tArrival time range\n").unwrap();
            let bps = (reply.rate * (PACKET_SIZE as f32)) * 8.0;
            let mbps = bps / 1024.0 / 1024.0;
            write!(
                screen,
                "{}/sec\t\t{}Mbps\t{}ms\n",
                reply.rate.round(),
                mbps,
                reply.delay
            )
            .unwrap();
            let rate_health = reply.rate / rate;
            let delay_health = (2000.0 - reply.delay) / 2000.0;
            progress_bar(&mut screen, rate_health);
            write!(screen, "\t\t").unwrap();
            progress_bar(&mut screen, delay_health);
            screen.flush().unwrap();
        } else {
            // Stale message received
        }
    }
}

fn server() -> std::io::Result<()> {
    {
        let socket = UdpSocket::bind("0.0.0.0:37131")?;

        let boot_time = Instant::now();

        println!(
            "{}{}Listening for incoming messages",
            termion::clear::All,
            termion::cursor::Goto(1, 1)
        );

        let mut received_packets = 0;
        let mut time = Instant::now();

        let mut receive_rate = MovingAverage::new(10);
        let mut clock_diff = MovingAverage::new(100);

        let mut recv_buf = vec![0; 8192];
        let mut send_buf = vec![0; 40];
        let mut seq = 0;

        loop {
            let (_amt, sender) = socket.recv_from(&mut recv_buf)?;
            let packet = ClientPacket::read(&mut recv_buf);
            received_packets += 1;

            clock_diff.add_sample(
                (((Instant::now() - boot_time).as_millis() as i64) - packet.time as i64) as f32,
            );

            let time_passed = Instant::now() - time;
            // 100 milliseconds passed, how many packets received?
            if time_passed > Duration::from_millis(100) {
                time = Instant::now();

                receive_rate.add_sample((received_packets as f32) / time_passed.as_secs_f32());

                print!("{}{}", termion::clear::All, termion::cursor::Goto(1, 1));
                println!("Rate\t\t\tarrival time range");
                let min_lag = clock_diff.min().unwrap_or(0.0);
                let rate = receive_rate.calculate_average().unwrap_or(0.0);
                let max_lag = clock_diff.max().unwrap_or(0.0) - min_lag;
                println!("{}/sec\t\t{}ms", rate.round(), max_lag,);
                let rate_health = rate / packet.send_rate;
                let delay_health = (2000.0 - max_lag) / 2000.0;
                progress_bar(&mut stdout(), rate_health);
                print!("\t\t");
                progress_bar(&mut stdout(), delay_health);

                let reply = ServerPacket {
                    rate,
                    delay: max_lag,
                    seq,
                };
                seq += 1;
                reply.write(&mut send_buf);
                socket
                    .send_to(&send_buf, sender)
                    .expect("couldn't send data");
                stdout().flush().unwrap();
                received_packets = 0;
            }
        }
    } // the socket is closed here
}

struct ClientPacket {
    time: u64,
    seq: u64,
    send_rate: f32,
}

impl ClientPacket {
    fn write(&self, buf: &mut [u8]) {
        NetworkEndian::write_u64(buf, self.time);
        NetworkEndian::write_u64(&mut buf[8..], self.seq);
        NetworkEndian::write_f32(&mut buf[16..], self.send_rate);
    }

    fn read(buf: &[u8]) -> Self {
        ClientPacket {
            time: NetworkEndian::read_u64(&buf[0..]),
            seq: NetworkEndian::read_u64(&buf[8..]),
            send_rate: NetworkEndian::read_f32(&buf[16..]),
        }
    }
}

struct ServerPacket {
    rate: f32,
    delay: f32,
    seq: u64,
}

impl ServerPacket {
    fn write(&self, buf: &mut [u8]) {
        NetworkEndian::write_f32(buf, self.rate);
        NetworkEndian::write_f32(&mut buf[4..], self.delay);
        NetworkEndian::write_u64(&mut buf[8..], self.seq);
    }

    fn read(buf: &[u8]) -> Self {
        ServerPacket {
            rate: NetworkEndian::read_f32(&buf[0..]),
            delay: NetworkEndian::read_f32(&buf[4..]),
            seq: NetworkEndian::read_u64(&buf[8..]),
        }
    }
}

struct MovingAverage {
    samples: Vec<f32>,
    next_index: usize,
}

impl MovingAverage {
    fn new(sample_count: usize) -> Self {
        MovingAverage {
            samples: Vec::with_capacity(sample_count),
            next_index: 0,
        }
    }

    fn add_sample(&mut self, sample: f32) {
        if self.next_index >= self.samples.capacity() {
            self.next_index = 0;
        }
        if self.next_index >= self.samples.len() {
            self.samples.push(sample);
        } else {
            self.samples[self.next_index] = sample;
        }

        self.next_index += 1;
    }

    fn calculate_average(&self) -> Option<f32> {
        let mut accum = 0.0;
        for sample in &self.samples {
            accum += sample;
        }

        Some(accum / (self.samples.len() as f32))
    }

    fn min(&self) -> Option<f32> {
        let mut current = None;
        for sample in &self.samples {
            current = match current {
                None => Some(*sample),
                Some(ref min) if sample < min => Some(*sample),
                Some(min) => Some(min),
            }
        }
        current
    }

    fn max(&self) -> Option<f32> {
        let mut current = None;
        for sample in &self.samples {
            current = match current {
                None => Some(*sample),
                Some(ref max) if sample > max => Some(*sample),
                Some(max) => Some(max),
            }
        }
        current
    }
}
