use std::{net::{TcpListener, TcpStream}, io, sync::{Arc, Mutex}, collections::VecDeque, };

use crate::{netcode::packets::{send_packet, ServerPacket, read_packet, ClientPacket,}, card::{Deck, Hand, Color}, game::verify_move};
use crate::netcode::misc::Names;
use crate::card;

use card::Card;

macro_rules! cls {
    () => {
        print!("\x1B[2J\x1b[1;1H");
    }
}

macro_rules! server_received_unexpected_packet {
    () => {
        bunt::println!("{$red}[{}]Server received unexpected packet from client{/$}", line!())
    };
}


macro_rules! client_received_unexpected_packet {
    () => {
        bunt::println!("{$red}Client received unexpected packet from server{/$}")
    };
}

#[derive(Debug, PartialEq, PartialOrd)]
enum GamePhase {
    Waiting,
    InGame,
    GameOver,
}

#[derive(Debug)]
enum Direction {
    Positive,
    Negative,
}

impl Direction {
    fn flip(&mut self) {
        *self = match *self {
            Direction::Positive => Direction::Negative,
            Direction::Negative => Direction::Positive,
        }
    }
}

#[derive(Debug)]
struct ClientInfo {
    stream: TcpStream,
    name: String,
    hand: Hand,
    is_active: bool,
}

#[derive(Debug)]
struct GlobalGameData {
    names : Names,
    game_phase: GamePhase,
    curr_total_clients_num : usize,
    curr_client_id_turn: usize, /// Number between 0 and curr_clients_num (non inclusive).
    master_deck : Deck,
    direction: Direction,
    card_debt: usize,
    skip_debt: usize,
    stack: VecDeque<Card>,
    clients_info: Vec<ClientInfo>,
}

impl GlobalGameData {
    fn get_players_string(&self) -> String {
        let mut ret_string = String::new();
        for client_idx in 0..self.clients_info.len() {
            if client_idx == self.curr_client_id_turn {
                ret_string += "* ";
            }
            else {
                ret_string += "  ";
            }
            ret_string += &format!("({}) ", self.clients_info[client_idx].hand.len()).to_string();
            ret_string += &self.clients_info[client_idx].name;
            ret_string += "\n"
        }
        ret_string
    }

    /// Goes to the next player after accounting for skip_debt, direction, and inactive players
    fn next_player(&mut self) {
        fn next_client_id_wrapping(global_game_data : &mut GlobalGameData) {
            let rhs = match global_game_data.direction {
                Direction::Positive => 1,
                Direction::Negative => -1,
            };
            global_game_data.curr_client_id_turn = 
                (global_game_data.curr_client_id_turn as isize + rhs)
                .rem_euclid(global_game_data.curr_total_clients_num as isize) as usize;
        }

        loop {
            next_client_id_wrapping(self);
            if !self.clients_info[self.curr_client_id_turn].is_active {
                continue;
            }
            if self.skip_debt > 0 {
                self.skip_debt = 0;
                continue;
            }
            break;
        }

        // let rhs : isize;
        // match self.direction {
        //     Direction::Positive => rhs = 1 + self.skip_debt as isize,
        //     Direction::Negative => rhs = - (1 + self.skip_debt as isize),
        // }
        // let next_player = (lhs + rhs).rem_euclid(self.curr_total_clients_num as isize);
        // if next_player < 0 || next_player >= self.curr_total_clients_num as isize {
        //     next_player = next_player % self.curr_total_clients_num as isize;
        // }
        // self.curr_client_id_turn = next_player as usize;
        // self.skip_debt = 0;
    }

    /// If game is over, returns Some(id) of client who lost, otherwise returns None
    fn is_game_over(&mut self) -> Option<usize> {
        let mut inactive_clients : usize = 0;
        let mut loser_client = 0; // last active client
        for client_idx in 0..self.clients_info.len() {
            let client = &self.clients_info[client_idx];
            if !client.is_active {
                inactive_clients += 1;
            }
            else {
                loser_client = client_idx;
            }
        }
        match inactive_clients == self.clients_info.len() - 1 {
            true => Some(loser_client),
            false => None
        }
    }
}

pub async fn run_server(port : u32) -> Result<(), Box<dyn std::error::Error>> {
    // let listener = ngrok::Session::builder()
    //     .authtoken_from_env()
    //     .connect()
    //     .await?
    //     .tcp_endpoint()
    //     .listen()
    //     .await?;

    // let tunnel_res = ngrok::NgrokTunnel::new(port);
    // if tunnel_res.is_err() {
    //     println!("{}", tunnel_res.unwrap_err());
    //     exit(1) //FIXME: Please use '?' and results in the project!!!
    // }
    // let mut tunnel = tunnel_res.unwrap();
    // println!("{}", tunnel.url());
    //
    // tokio::spawn(async move {
    //     let res = tunnel.process_handle.wait();
    //     dbg!(res);
    // });

    bunt::println!("{$green}The server has been started{/$}");
    let listener = TcpListener::bind(format!("localhost:{port}"))?;
    let mut deck = Deck::new();
    let mut stack_card;
    loop {
        stack_card = deck.pop_random_card();
        match stack_card.kind {
            card::CardKind::Wild | card::CardKind::Draw4 => deck.push_card(stack_card),
            _ => break,
        }
    }
    
    let mut skip_debt = 0;
    let mut card_debt = 0;
    let mut direction = Direction::Positive;
    match stack_card.kind {
        card::CardKind::Number => {},
        card::CardKind::Skip => skip_debt = 1,
        card::CardKind::Reverse => direction.flip(),
        card::CardKind::Draw2 => card_debt += 2,
        card::CardKind::Draw4 | card::CardKind::Wild => unreachable!(),
    }
    let shared_global_game_data = Arc::new(Mutex::new(GlobalGameData {
        names: Names::new(),
        game_phase: GamePhase::Waiting,
        curr_total_clients_num: 0, /// Total number of connected clients
        curr_client_id_turn:0, /// Number between 0 and curr_clients_num (non inclusive).
        master_deck: deck, /// The main deck from where cards are taken to form hands
        direction, // Two directions in which the game goes. Changes when reverse card is used
        card_debt,
        skip_debt,
        stack: VecDeque::from(vec![stack_card]),
        clients_info: vec![],
    }));

    /// IMP: This function holds shared_state for a long time
    async fn game_thread(shared_state: Arc<Mutex<GlobalGameData>>) {
        let mut shrared_state_held = shared_state.lock().unwrap();
        shrared_state_held.game_phase = GamePhase::InGame;
        loop {
            // provide updates to players
            for idx in 0..shrared_state_held.clients_info.len() {
                if !shrared_state_held.clients_info[idx].is_active {continue;}
                let mut msg_first_half = "\nPlayers: \n".to_string() + &shrared_state_held.get_players_string() + "\n";
                msg_first_half += &format!("Topmost card: {}\n", shrared_state_held.stack.get(0).unwrap().get_colorized_repr()).to_string();
                let msg_second_half;
                if shrared_state_held.card_debt > 0 {
                    msg_second_half
                        = format!("Type number 1-{} to choose the card at that index as indicated above in your hand. \
                                  If choosing a Draw4 or Wild, tell the color as well (eg: `2 blue` given 2 has a Draw4 or a Wild). \
                                  You can only choose a Draw2 or a Draw4 to make the next opponent pick up {} or {} cards respectively. \
                                  You can also type 'p' to pick up {} cards",
                                  shrared_state_held.clients_info[idx].hand.len(), shrared_state_held.card_debt + 2, shrared_state_held.card_debt + 4, shrared_state_held.card_debt);
                } 
                else {
                    msg_second_half = format!("Type number 1-{} to choose the card at that index as indicated above in your hand. \
                                              If choosing a Draw4 or Wild, type the chosen color as well (eg: `2 blue` given 2 has a Draw4 or a Wild). \
                                              You can also type 'p' to pick up 1 card",
                    shrared_state_held.clients_info[idx].hand.len());
                };
                let hand_copy = shrared_state_held.clients_info[idx].hand.clone();
                let is_my_turn = idx == shrared_state_held.curr_client_id_turn;
                send_packet(&mut shrared_state_held.clients_info[idx].stream,
                            ServerPacket::SendMsgUpdate { msg_first_half, hand: hand_copy, msg_second_half, is_my_turn });
            }
            let curr_client_id = shrared_state_held.curr_client_id_turn;
            let client_send_move_packet = read_packet::<ClientPacket>(&mut shrared_state_held.clients_info[curr_client_id].stream);
            match client_send_move_packet {
                ClientPacket::SendMoveCard { card_idx, color_choice } => {
                    let mut card = shrared_state_held.clients_info[curr_client_id].hand.get_at(card_idx);
                    if color_choice.is_some() {card.set_draw4_or_wild_color(color_choice.unwrap())}; // In case Wild or Draw4, need to set color
                    let result = verify_move(card.clone(), shrared_state_held.stack.get(0).unwrap().clone(), shrared_state_held.card_debt);
                    match result {
                        Ok(_) => {
                            match card.kind {
                                card::CardKind::Number => {},
                                card::CardKind::Skip => shrared_state_held.skip_debt = 1,
                                card::CardKind::Reverse => shrared_state_held.direction.flip(),
                                card::CardKind::Draw2 => shrared_state_held.card_debt += 2,
                                card::CardKind::Draw4 => shrared_state_held.card_debt += 4,
                                card::CardKind::Wild => {},
                            }
                            let mut card = shrared_state_held.clients_info[curr_client_id].hand.pop_at(card_idx);
                            if color_choice.is_some() {card.set_draw4_or_wild_color(color_choice.unwrap())}; // In case Wild or Draw4, need to set color
                            shrared_state_held.stack.push_front(card.clone());
                            shrared_state_held.master_deck.push_card(card);
                            shrared_state_held.next_player();
                            send_packet(&mut shrared_state_held.clients_info[curr_client_id].stream, ServerPacket::SendMoveAcknowledgement { msg: None });

                            // check if player won
                            if shrared_state_held.clients_info[curr_client_id].hand.len() == 0 {
                                send_packet(&mut shrared_state_held.clients_info[curr_client_id].stream, ServerPacket::YouWon);
                                shrared_state_held.clients_info[curr_client_id].is_active = false;
                                match shrared_state_held.is_game_over() {
                                    Some(id) => {
                                        bunt::println!("{$yellow}All but one clients are inactive, Game Over!{/$}");
                                        send_packet(&mut shrared_state_held.clients_info[id].stream, ServerPacket::YouLost);
                                        shrared_state_held.game_phase = GamePhase::GameOver;
                                        return;
                                    }
                                    None => {},
                                }
                                // shrared_state_held.clients_info.remove(curr_client_id);
                            }
                        }
                        Err(e) => {
                            send_packet(&mut shrared_state_held.clients_info[curr_client_id].stream, ServerPacket::SendMoveAcknowledgement { msg: Some(e) });
                        }
                    }
                }
                ClientPacket::SendMovePick => {
                    let pick_up_amt = if shrared_state_held.card_debt > 0 {shrared_state_held.card_debt} else {1};
                    for _ in 0..pick_up_amt {
                        let card = shrared_state_held.master_deck.pop_random_card();
                        shrared_state_held.clients_info[curr_client_id].hand.push(card);
                    }
                    shrared_state_held.next_player();
                    shrared_state_held.card_debt = 0;
                }
                _ => server_received_unexpected_packet!(),
            }
        }
    }

    /*
     * server commands thread: Executes commands sent to the server.
     */
    tokio::spawn({
        let shared_state = shared_global_game_data.clone();
        // let tx = tx.clone();
        async move {
            let mut input_line;
            let possible_commands = vec!["start", "clients_info"].iter().map(|elem| elem.to_string()).collect::<Vec<String>>();
            loop {
                input_line = String::new();
                std::io::stdin().read_line(&mut input_line).unwrap();
                input_line = input_line.trim().to_lowercase();
                if !possible_commands.contains(&input_line) {
                    bunt::println!("{$red}Unrecongized command. Valid commnads are: {:?} {/$}", possible_commands);
                }
                else if input_line == "clients_info" {
                    let shared_state_locked = shared_state.lock().unwrap();
                    dbg!(&shared_state_locked.clients_info);
                }
                else if input_line == "start" {
                    if shared_state.lock().unwrap().game_phase != GamePhase::Waiting {
                        bunt::println!("{$red}Game cannot be started if it already has started{/$}");
                    }
                    else if shared_state.lock().unwrap().curr_total_clients_num < 2 {
                        bunt::println!("{$red}Game cannot be started if number of players less than 2{/$}");
                    }
                    else {
                        // tx.send(GameThreadBroadcastPacket::StartGame).unwrap();
                        bunt::println!("{$magenta}Game Started!{/$}");
                        // FIXME: Can possibly run this async (without await). To do that, need to
                        // not hold mutex throughout the lifetime of game_thread()...
                        tokio::spawn(game_thread(shared_state.clone())).await.unwrap();
                        if shared_state.lock().unwrap().game_phase == GamePhase::GameOver {
                            bunt::println!("{$yellow}Game has ended. Thanks for playing! :){/$}");
                            return Ok::<(), ()>(())
                        }
                    }
                }
            }
        }
    });

    /* 
     * Client serving server thread: A server thread (per client) to serve the client. All these
     * threads "talk" with the 
     */
    loop {
        let (mut stream, _) = listener.accept()?;
        let shared_state = shared_global_game_data.clone();

        // for every new connection
        tokio::spawn(async move {

        // ==== Setting Client Name ====
        send_packet(&mut stream, ServerPacket::AskPreferredName);
        // Wait for client to send thier name
        let mut ret_name = "Unnamed".to_string();
        match read_packet::<ClientPacket>(&mut stream) {
            ClientPacket::SendPreferredName { optional_client_name} => {
                let ret_msg;
                match optional_client_name {
                    Some(name) => {
                        let specific_name_result = shared_state.lock().unwrap().names.get_specific_name(name);
                        match specific_name_result {
                            Ok(name) => {ret_name = name; ret_msg = None;}
                            Err(()) => {
                                ret_name = shared_state.lock().unwrap().names.get_random_name();
                                ret_msg = Some("Provided name was invalid, hence random name assigned. Names must not have whitespaces and the '#' char. You may reconnect with a valid name.".to_string());
                            }
                        }
                    }
                    None => {ret_name = shared_state.lock().unwrap().names.get_random_name();ret_msg = None}
                }
                send_packet(&mut stream, ServerPacket::SendGivenName { name: ret_name.clone(), optional_msg: ret_msg });
                {
                    let mut locked_game_data = shared_state.lock().unwrap();
                    locked_game_data.curr_total_clients_num += 1;
                    let hand = Hand::new(7, &mut locked_game_data.master_deck); //TODO: let users
                                                                                //decide how many
                                                                                //cards to start
                                                                                //with
                    locked_game_data.clients_info.push(ClientInfo {
                        name: ret_name.clone(), hand, stream, is_active: true,
                    })
                }
            }
            _ => server_received_unexpected_packet!(),
        }
        bunt::println!("{$green}{} has joined the game!{/$}", ret_name);
        // At this point, the client has connected to the server!
        });
    }
}

pub async fn run_client(optional_client_name : Option<&String>, join_code_: String) -> Result<(), Box<dyn std::error::Error>> {
    let join_code_pair = join_code_.split_at(1);
    let addr = format!("{}.tcp.ngrok.io:{}", join_code_pair.0, join_code_pair.1);
    let mut stream = TcpStream::connect(addr)?;
    match read_packet::<ServerPacket>(&mut stream) {
        ServerPacket::AskPreferredName => {
            send_packet(&mut stream, ClientPacket::SendPreferredName { optional_client_name: optional_client_name.cloned() })
        }
        _ => client_received_unexpected_packet!()
    }

    match read_packet::<ServerPacket>(&mut stream) {
        ServerPacket::SendGivenName { name, optional_msg } => {
            match optional_msg {
                Some(msg) => {bunt::println!("{$red}{}{/$}", msg)}
                None => (),
            }
            bunt::println!("{$green}Your name is: {}{/$}", name)
        }
        _ => client_received_unexpected_packet!()
    }

    // At this point, the client has connected to the server!
    cls!();
    loop {
        let packet = read_packet::<ServerPacket>(&mut stream);
        match packet {
            ServerPacket::SendMsgUpdate { msg_first_half, hand, msg_second_half, is_my_turn } => {
                println!("{}", msg_first_half);
                println!("{}", hand);
                match is_my_turn {
                    // FIXME: Kinda rework on this. Implement case for 2 blue etc.
                    true => {
                        print!("It is your turn! ");
                        println!("{}", msg_second_half);
                        loop {
                            let mut input_str : String = "".to_string();
                            io::stdin().read_line(&mut input_str).expect("FATAL ERROR: Could not read line");
                            let mut input_words = input_str.split_whitespace();
                            let first_input = input_words.next(); // Must be either a number or 'p'
                            if first_input.is_none() {bunt::println!("{$red}Invalid Input, try again:{/$}"); continue;}
                            match first_input.unwrap().trim().parse::<usize>() {
                                // we were able to parse the first_input as a number and the idx is
                                // in a valid range
                                Ok(card_idx) if card_idx > 0 && card_idx <= hand.len() => {
                                    match hand.get_at(card_idx).kind {
                                        card::CardKind::Draw4 | card::CardKind::Wild  => {
                                            let second_input = input_words.next();
                                            if second_input.is_none() {
                                                bunt::println!("{$red}Invalid Input. \
                                                               You must include a color when choosing the Draw4 or wild card, try again:{/$}");
                                                continue;
                                            }
                                            let chosen_color : Color = match second_input.unwrap().chars().next() {
                                                Some(c) if c.to_ascii_lowercase() == 'r' => Color::Red,
                                                Some(c) if c.to_ascii_lowercase() == 'g' => Color::Green,
                                                Some(c) if c.to_ascii_lowercase() == 'b' => Color::Blue,
                                                Some(c) if c.to_ascii_lowercase() == 'y' => Color::Yellow,
                                                Some(_) => {bunt::println!("{$red}Invalid Input. Could not parse color choice. Try again:{/$}"); continue;}
                                                None => {bunt::println!("{$red}Invalid Input, try again:{/$}"); continue;}
                                            };
                                            send_packet(&mut stream, ClientPacket::SendMoveCard { card_idx, color_choice: Some(chosen_color) });
                                            cls!();
                                            break;
                                    }
                                    _ => {
                                        send_packet(&mut stream, ClientPacket::SendMoveCard { card_idx, color_choice: None});
                                        cls!();
                                        break;
                                    }
                                }
                            }
                                Ok(card_idx) if !(card_idx > 0 && card_idx <= hand.len()) => {bunt::println!("{$red}Invalid Input. Card index not in range! try again:{/$}"); continue;}
                                _ => {
                                    if input_str.trim().to_lowercase() == *"p" {
                                        send_packet(&mut stream, ClientPacket::SendMovePick);
                                        cls!();
                                        break;
                                    }
                                    // Not a number, not 'p', but also not whitespace
                                    else {
                                        bunt::println!("{$red}Invalid Input, try again:{/$}");
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    false => {println!("It is not your turn."); cls!();},
                }
            }
            ServerPacket::SendMoveAcknowledgement { msg } => {
                match msg {
                    Some(msg) => bunt::println!("{$red}{}{/$}", msg),
                    None => (),
                }
            }
            ServerPacket::YouWon => {
                bunt::println!("{$yellow}You Won!!{/$}");
                break;
            }
            ServerPacket::YouLost => {
                bunt::println!("{$red}You Lost.{/$}");
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
