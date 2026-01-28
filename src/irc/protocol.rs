#[derive(Debug, Clone)]
pub enum Command {
    Cap(String),
    Nick(String),
    User { username: String, realname: String },
    Join(String),
    Part(String),
    Privmsg { target: String, message: String },
    Ping(String),
    Pong(String),
    Quit(Option<String>),
    Unknown(String),
}

impl Command {
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        let mut parts = line.splitn(2, ' ');
        let cmd = parts.next()?.to_uppercase();
        let rest = parts.next().unwrap_or("");

        Some(match cmd.as_str() {
            "CAP" => Command::Cap(rest.to_string()),
            "NICK" => Command::Nick(rest.trim_start_matches(':').to_string()),
            "USER" => {
                let mut parts = rest.splitn(4, ' ');
                let username = parts.next().unwrap_or("").to_string();
                let _ = parts.next(); // mode
                let _ = parts.next(); // unused
                let realname = parts.next().unwrap_or("").trim_start_matches(':').to_string();
                Command::User { username, realname }
            }
            "JOIN" => {
                let channel = rest.split_whitespace().next().unwrap_or("").trim_start_matches(':');
                Command::Join(channel.to_string())
            }
            "PART" => Command::Part(rest.splitn(2, ' ').next().unwrap_or("").to_string()),
            "PRIVMSG" => {
                let mut parts = rest.splitn(2, ' ');
                let target = parts.next().unwrap_or("").to_string();
                let message = parts.next().unwrap_or("").trim_start_matches(':').to_string();
                Command::Privmsg { target, message }
            }
            "PING" => Command::Ping(rest.trim_start_matches(':').to_string()),
            "PONG" => Command::Pong(rest.trim_start_matches(':').to_string()),
            "QUIT" => {
                let msg = if rest.is_empty() {
                    None
                } else {
                    Some(rest.trim_start_matches(':').to_string())
                };
                Command::Quit(msg)
            }
            _ => Command::Unknown(line.to_string()),
        })
    }
}

pub struct Reply;

impl Reply {
    pub fn cap_ls(server: &str) -> String {
        format!(":{} CAP * LS :\r\n", server)
    }

    pub fn cap_end(server: &str) -> String {
        format!(":{} CAP * ACK :\r\n", server)
    }

    pub fn welcome(nick: &str, server: &str) -> String {
        format!(":{} 001 {} :Welcome to {} {}\r\n", server, nick, server, nick)
    }

    pub fn join(nick: &str, channel: &str, server: &str) -> String {
        format!(":{}!{}@{} JOIN {}\r\n", nick, nick, server, channel)
    }

    pub fn part(nick: &str, channel: &str, server: &str) -> String {
        format!(":{}!{}@{} PART {}\r\n", nick, nick, server, channel)
    }

    pub fn privmsg(nick: &str, target: &str, message: &str, server: &str) -> String {
        format!(":{}!{}@{} PRIVMSG {} :{}\r\n", nick, nick, server, target, message)
    }

    pub fn pong(server: &str, token: &str) -> String {
        format!(":{} PONG {} :{}\r\n", server, server, token)
    }

    pub fn namreply(nick: &str, channel: &str, names: &[String], server: &str) -> String {
        format!(":{} 353 {} = {} :{}\r\n", server, nick, channel, names.join(" "))
    }

    pub fn endofnames(nick: &str, channel: &str, server: &str) -> String {
        format!(":{} 366 {} {} :End of /NAMES list\r\n", server, nick, channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nick() {
        let cmd = Command::parse("NICK alice").unwrap();
        match cmd {
            Command::Nick(n) => assert_eq!(n, "alice"),
            _ => panic!("expected Nick"),
        }
    }

    #[test]
    fn test_parse_user() {
        let cmd = Command::parse("USER alice 0 * :Alice Smith").unwrap();
        match cmd {
            Command::User { username, realname } => {
                assert_eq!(username, "alice");
                assert_eq!(realname, "Alice Smith");
            }
            _ => panic!("expected User"),
        }
    }

    #[test]
    fn test_parse_join() {
        let cmd = Command::parse("JOIN #general").unwrap();
        match cmd {
            Command::Join(c) => assert_eq!(c, "#general"),
            _ => panic!("expected Join"),
        }
    }

    #[test]
    fn test_parse_privmsg() {
        let cmd = Command::parse("PRIVMSG #general :hello world").unwrap();
        match cmd {
            Command::Privmsg { target, message } => {
                assert_eq!(target, "#general");
                assert_eq!(message, "hello world");
            }
            _ => panic!("expected Privmsg"),
        }
    }

    #[test]
    fn test_parse_ping() {
        let cmd = Command::parse("PING :12345").unwrap();
        match cmd {
            Command::Ping(t) => assert_eq!(t, "12345"),
            _ => panic!("expected Ping"),
        }
    }

    #[test]
    fn test_parse_empty() {
        assert!(Command::parse("").is_none());
        assert!(Command::parse("   ").is_none());
    }

    #[test]
    fn test_reply_privmsg() {
        let reply = Reply::privmsg("alice", "#general", "hello", "abbot");
        assert_eq!(reply, ":alice!alice@abbot PRIVMSG #general :hello\r\n");
    }

    #[test]
    fn test_reply_pong() {
        let reply = Reply::pong("abbot", "12345");
        assert_eq!(reply, ":abbot PONG abbot :12345\r\n");
    }
}
