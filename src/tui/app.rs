use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::bus::{respond, Message, MessageData, MessageOp, Origin, Scope, TaskMsg};
use crate::socket::WireMessage;

use super::ui;

#[derive(Clone, Debug)]
pub enum TaskStatus {
    Requested,
    Assigned(String),
    InProgress(String),
    Done(bool, String),
}

#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub task_id: String,
    pub goal: String,
    pub status: TaskStatus,
}

pub struct App {
    socket_path: String,
    scope: Scope,
    messages: Vec<Message>,
    tasks: HashMap<String, TaskInfo>,
    activity: HashMap<String, usize>,
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
            tasks: HashMap::new(),
            activity: HashMap::new(),
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
                self.handle_message(msg);
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

    fn handle_message(&mut self, msg: Message) {
        let msg_scope = msg.scope.to_string();
        let is_current_scope = msg_scope == self.scope.to_string();

        if msg.op == MessageOp::Task {
            if let MessageData::Task(ref task_msg) = msg.data {
                self.update_task(task_msg);
            }
        }

        if is_current_scope {
            self.messages.push(msg);
        } else {
            *self.activity.entry(msg_scope).or_insert(0) += 1;
        }
    }

    fn update_task(&mut self, task_msg: &TaskMsg) {
        match task_msg {
            TaskMsg::Request { task_id, goal, .. } => {
                self.tasks.insert(
                    task_id.clone(),
                    TaskInfo {
                        task_id: task_id.clone(),
                        goal: goal.clone(),
                        status: TaskStatus::Requested,
                    },
                );
            }
            TaskMsg::Assigned {
                task_id, hand_id, ..
            } => {
                if let Some(task) = self.tasks.get_mut(task_id) {
                    task.status = TaskStatus::Assigned(hand_id.clone());
                }
            }
            TaskMsg::Progress { task_id, note, .. } => {
                if let Some(task) = self.tasks.get_mut(task_id) {
                    task.status = TaskStatus::InProgress(note.clone());
                }
            }
            TaskMsg::Echo { task_id, tool, .. } => {
                if let Some(task) = self.tasks.get_mut(task_id) {
                    task.status = TaskStatus::InProgress(format!("running {}", tool));
                }
            }
            TaskMsg::Result {
                task_id,
                ok,
                summary,
                ..
            } => {
                if let Some(task) = self.tasks.get_mut(task_id) {
                    task.status = TaskStatus::Done(*ok, summary.clone());
                }
            }
        }
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn tasks(&self) -> &HashMap<String, TaskInfo> {
        &self.tasks
    }

    pub fn activity(&self) -> &HashMap<String, usize> {
        &self.activity
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }
}
