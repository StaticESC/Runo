use std::{net::{TcpListener, TcpStream}, io};
use rand::Rng;

use crate::netcode::packets::{send_packet, ServerPacket, read_packet, ClientPacket};

const MAX_PLAYERS_LIMIT : u8 = 10;
macro_rules! cls {
    () => {
        print!("\x1B[2J\x1b[1;1H");
    }
}

macro_rules! server_received_unexpected_packet {
    () => {
        bunt::println!("{$red}Server received unexpected packet from client{/$}")
    };
}

pub async fn run_server(port : u32, server_is_open : bool) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))?;
    let mut rng = rand::thread_rng();
    bunt::println!("{$green}The server has been started{/$}");
    let join_code = rng.gen_range(100_000..999_000);
    if !server_is_open {
        bunt::println!("{$green}Joining code is: {} {/$}", join_code);
    }

    /*
     * Main server loop:
     * Logs events from self (server) and clients
     * Admin user can also run commands
     */



    /* Client serving loop 
     * 1 thread per client
     */
    loop {
        let (mut stream, peer_addr) = listener.accept()?;

        // for every new connection
        tokio::spawn(async move {
            match server_is_open {
                true => {
                    bunt::println!("{$green}A client connected!{/$}");
                    send_packet(&mut stream, ServerPacket::AuthRequest {required: false});
                }
                false => {
                    // try authenticating the client until it is authenticated
                    let mut is_client_authenticated = false;
                    bunt::println!("{$yellow}A Client is attemtping to join...{/$}");
                    //TODO: Consider adding "number of tries" for the client to join in
                    while !is_client_authenticated {
                        send_packet(&mut stream, ServerPacket::AuthRequest {required: true});
                        match read_packet::<ClientPacket>(&mut stream) {
                            ClientPacket::AuthResponse { join_code: code } if code == join_code => {
                                bunt::println!("{$green}A client was successfully authentiated and connected!{/$}");
                                is_client_authenticated = true;
                                send_packet(&mut stream, ServerPacket::AuthAcknowledged)
                            }
                            ClientPacket::AuthResponse { join_code:code } if code != join_code => {}
                            _ => server_received_unexpected_packet!()
                        }
                    }
                }
            }
            // At this point the client has connected!

        });
    }
}

pub async fn run_client(port : u32) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(format!("0.0.0.0:{}", port))?;
    // Auth loop. Keeps on going if client gives wrong join_code. Ends when it gives right join_code
    let mut is_retry = false;
    loop {
        match read_packet::<ServerPacket>(&mut stream) {
            ServerPacket::AuthRequest { required } => {
                match required {
                    true => {
                        if is_retry {bunt::println!("{$red}Wrong join code{/$}")}
                        bunt::println!("{$yellow}Please provide the join code generated by the server: {/$}");
                        loop {
                            let mut code_str = String::new();
                            io::stdin().read_line(&mut code_str).expect("FATAL ERROR: Could not read line");
                            match code_str.trim().parse::<usize>() {
                                Ok(join_code) => {
                                    send_packet(&mut stream, ClientPacket::AuthResponse { join_code });
                                    is_retry = true;
                                    break;
                                }
                                Err(_) => bunt::println!("{$red}Could not parse input, try again:{/$}")
                            }
                        }
                    }
                    false => {bunt::println!("{$green}Successfully connected to server!{/$}");break;}
                }
            }
            ServerPacket::AuthAcknowledged => {
                bunt::println!("{$green}Successfully connected to server!{/$}");
                break;
            }
            _ => server_received_unexpected_packet!()
        }
    }

    Ok(())
}