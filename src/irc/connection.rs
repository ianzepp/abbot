use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, RwLock, mpsc};
use crate::bus::{Hub, Message, MessageOp, respond};
use super::protocol::{Command, Reply};
use super::format::markdown_to_irc;

const SERVER_NAME: &str = "abbot";

struct State {
    nick: Option<String>,
    registered: bool,
    channels: Vec<String>,
}

pub struct Connection {
    stream: TcpStream,
    hub: Arc<RwLock<Hub>>,
}

impl Connection {
    pub fn new(stream: TcpStream, hub: Arc<RwLock<Hub>>) -> Self {
        Self { stream, hub }
    }

    pub async fn run(self) {
        let hub = self.hub;
        let (reader, mut writer) = self.stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        let (tx, mut rx) = mpsc::channel::<String>(256);

        let mut state = State {
            nick: None,
            registered: false,
            channels: Vec::new(),
        };

        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if writer.write_all(msg.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        let mut hub_receivers: Vec<(String, broadcast::Receiver<Message>)> = Vec::new();

        loop {
            tokio::select! {
                result = reader.read_line(&mut line) => {
                    match result {
                        Ok(0) => break,
                        Ok(_) => {
                            if let Some(cmd) = Command::parse(&line) {
                                if let Some(response) = handle_command(&hub, &mut state, cmd, &mut hub_receivers).await {
                                    let _ = tx.send(response).await;
                                }
                            }
                            line.clear();
                        }
                        Err(_) => break,
                    }
                }
                _ = check_hub_messages(&state.nick, &tx, &mut hub_receivers) => {}
            }
        }

        writer_handle.abort();
    }
}

async fn check_hub_messages(
    nick: &Option<String>,
    tx: &mpsc::Sender<String>,
    receivers: &mut Vec<(String, broadcast::Receiver<Message>)>,
) {
    for (channel, rx) in receivers.iter_mut() {
        match rx.try_recv() {
            Ok(msg) => {
                // Skip internal responses (tool results, etc.)
                if msg.reply_to.is_some() {
                    continue;
                }

                if nick.as_ref().map(|n| n != &msg.sender).unwrap_or(true) {
                    let text = match msg.op {
                        MessageOp::Chat | MessageOp::Ok | MessageOp::Item => {
                            msg.text().unwrap_or("").to_string()
                        }
                        MessageOp::Error => {
                            if let crate::bus::MessageData::Error { code, message } = &msg.data {
                                format!("error [{}]: {}", code, message)
                            } else {
                                "error".to_string()
                            }
                        }
                        MessageOp::Progress => {
                            if let crate::bus::MessageData::Progress { percent, .. } = &msg.data {
                                format!("progress: {:.0}%", percent)
                            } else {
                                continue;
                            }
                        }
                        MessageOp::Done | MessageOp::Event | MessageOp::Data | MessageOp::Exec | MessageOp::Ping => continue,
                    };

                    if !text.is_empty() {
                        let formatted = markdown_to_irc(&text);
                        let reply = Reply::privmsg(&msg.sender, channel, &formatted, SERVER_NAME);
                        let _ = tx.send(reply).await;
                    }
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => {}
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(broadcast::error::TryRecvError::Closed) => {}
        }
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
}

async fn handle_command(
    hub: &Arc<RwLock<Hub>>,
    state: &mut State,
    cmd: Command,
    receivers: &mut Vec<(String, broadcast::Receiver<Message>)>,
) -> Option<String> {
    match cmd {
        Command::Cap(args) => {
            if args.starts_with("LS") {
                Some(Reply::cap_ls(SERVER_NAME))
            } else if args.starts_with("END") {
                None
            } else {
                None
            }
        }
        Command::Nick(nick) => {
            state.nick = Some(nick);
            try_register(state)
        }
        Command::User { .. } => {
            state.registered = true;
            try_register(state)
        }
        Command::Join(channel) => {
            let channel = if channel.starts_with('#') {
                channel
            } else {
                format!("#{}", channel)
            };

            hub.write().await.create_channel(&channel);

            if let Some(rx) = hub.read().await.subscribe(&channel) {
                receivers.push((channel.clone(), rx));
            }

            state.channels.push(channel.clone());

            let nick = state.nick.as_deref().unwrap_or("*");
            let mut response = Reply::join(nick, &channel, SERVER_NAME);

            let names: Vec<String> = state.channels.iter()
                .filter(|c| *c == &channel)
                .map(|_| nick.to_string())
                .collect();
            response.push_str(&Reply::namreply(nick, &channel, &names, SERVER_NAME));
            response.push_str(&Reply::endofnames(nick, &channel, SERVER_NAME));

            Some(response)
        }
        Command::Part(channel) => {
            state.channels.retain(|c| c != &channel);
            receivers.retain(|(c, _)| c != &channel);
            let nick = state.nick.as_deref().unwrap_or("*");
            Some(Reply::part(nick, &channel, SERVER_NAME))
        }
        Command::Privmsg { target, message } => {
            if let Some(nick) = &state.nick {
                if message.starts_with('!') {
                    if let Some((tool, args)) = parse_command(&message) {
                        let msg = respond::exec(nick, &target, tool, args);
                        let exec_id = msg.id;

                        let rx = hub.read().await.subscribe(&target);
                        hub.read().await.publish(&target, msg);

                        if let Some(mut rx) = rx {
                            let mut results = Vec::new();
                            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);

                            loop {
                                if tokio::time::Instant::now() > deadline {
                                    results.push("(timeout)".to_string());
                                    break;
                                }

                                match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                                    Ok(Ok(msg)) => {
                                        if msg.reply_to != Some(exec_id) {
                                            continue;
                                        }

                                        match msg.op {
                                            MessageOp::Item | MessageOp::Ok => {
                                                if let Some(text) = msg.text() {
                                                    if !text.is_empty() {
                                                        results.push(text.to_string());
                                                    }
                                                }
                                                if msg.op == MessageOp::Ok {
                                                    break;
                                                }
                                            }
                                            MessageOp::Error => {
                                                if let crate::bus::MessageData::Error { code, message } = &msg.data {
                                                    results.push(format!("error [{}]: {}", code, message));
                                                }
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                    Ok(Err(_)) => break,
                                    Err(_) => continue,
                                }
                            }

                            if !results.is_empty() {
                                let output = results.join("\n");
                                return Some(Reply::privmsg("tools", &target, &output, SERVER_NAME));
                            }
                        }
                    } else {
                        let msg = respond::chat(nick, &target, &message);
                        hub.read().await.publish(&target, msg);
                    }
                } else {
                    let msg = respond::chat(nick, &target, &message);
                    hub.read().await.publish(&target, msg);
                }
            }
            None
        }
        Command::Ping(token) => Some(Reply::pong(SERVER_NAME, &token)),
        Command::Pong(_) => None,
        Command::Quit(_) => None,
        Command::Unknown(_) => None,
    }
}

fn parse_command(message: &str) -> Option<(&str, &str)> {
    let message = message.trim();
    if !message.starts_with('!') {
        return None;
    }

    let without_bang = &message[1..];
    let mut parts = without_bang.splitn(2, ' ');
    let cmd = parts.next()?;
    let args = parts.next().unwrap_or("");

    Some((cmd, args))
}

fn try_register(state: &State) -> Option<String> {
    if state.nick.is_some() && state.registered {
        let nick = state.nick.as_deref().unwrap();
        Some(Reply::welcome(nick, SERVER_NAME))
    } else {
        None
    }
}
