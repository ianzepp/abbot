use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::bus::{respond, Message, Origin, Scope};
use crate::socket::WireMessage;

use super::ui;

pub struct App {
    socket_path: String,
    scope: Scope,
    messages: Vec<Message>,
    input: String,
    scroll_offset: usize,
    should_quit: bool,
}

impl App {
    pub fn new(socket_path: String, scope: String) -> Self {
        Self {
            socket_path,
            scope: Scope::from(scope.as_str()),
            messages: Vec::new(),
            input: String::new(),
            scroll_offset: 0,
            should_quit: false,
        }
    }

    pub fn run(mut self, mut terminal: DefaultTerminal) -> std::io::Result<()> {
        let stream = UnixStream::connect(&self.socket_path)?;
        stream.set_nonblocking(false)?;
        let mut writer = stream.try_clone()?;

        let (tx, rx) = mpsc::channel::<Message>();

        let reader_stream = stream;
        thread::spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if let Ok(wire) = serde_json::from_str::<WireMessage>(&line) {
                    if let Ok(msg) = Message::try_from(wire) {
                        let _ = tx.send(msg);
                    }
                }
            }
        });

        while !self.should_quit {
            while let Ok(msg) = rx.try_recv() {
                if msg.scope.to_string() == self.scope.to_string() {
                    self.messages.push(msg);
                }
            }

            terminal.draw(|frame| ui::draw(frame, &self))?;

            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            self.should_quit = true;
                        }
                        (KeyCode::Enter, _) => {
                            if !self.input.trim().is_empty() {
                                let msg = respond::chat("_user", self.scope.clone(), &self.input)
                                    .with_origin(Origin::Human);
                                let wire = WireMessage::from(&msg);
                                if let Ok(json) = serde_json::to_string(&wire) {
                                    let _ = writeln!(writer, "{}", json);
                                }
                                self.input.clear();
                            }
                        }
                        (KeyCode::Backspace, _) => {
                            self.input.pop();
                        }
                        (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                            self.input.push(c);
                        }
                        (KeyCode::Up, _) => {
                            if self.scroll_offset < self.messages.len().saturating_sub(1) {
                                self.scroll_offset += 1;
                            }
                        }
                        (KeyCode::Down, _) => {
                            if self.scroll_offset > 0 {
                                self.scroll_offset -= 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }
}
