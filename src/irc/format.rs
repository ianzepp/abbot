const BOLD: &str = "\x02";
const ITALIC: &str = "\x1D";
const RESET: &str = "\x0F";
const GREY: &str = "\x0314";

pub fn tool_notice(tool: &str, args: &str) -> String {
    let args_preview: String = args.chars().take(50).collect();
    format!("{}{}({}){}", GREY, tool, args_preview, RESET)
}

pub fn markdown_to_irc(text: &str) -> String {
    let mut result = text.to_string();

    // Bold: **text** or __text__
    result = replace_markdown(&result, "**", BOLD);
    result = replace_markdown(&result, "__", BOLD);

    // Italic: *text* or _text_ (but not inside words)
    result = replace_markdown_italic(&result);

    // Inline code: `code` - leave as-is, just remove backticks
    result = result.replace('`', "");

    result
}

fn replace_markdown(text: &str, marker: &str, code: &str) -> String {
    let mut result = String::new();
    let mut parts = text.split(marker);

    if let Some(first) = parts.next() {
        result.push_str(first);
    }

    let mut inside = false;
    for part in parts {
        if inside {
            result.push_str(code);
            result.push_str(RESET);
        } else {
            result.push_str(code);
        }
        result.push_str(part);
        inside = !inside;
    }

    result
}

fn replace_markdown_italic(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    let mut inside_italic = false;

    while let Some(c) = chars.next() {
        if (c == '*' || c == '_') && !inside_italic {
            // Check if this looks like start of italic (not **)
            if chars.peek() != Some(&c) {
                result.push_str(ITALIC);
                inside_italic = true;
                continue;
            }
        } else if (c == '*' || c == '_') && inside_italic {
            result.push_str(RESET);
            inside_italic = false;
            continue;
        }
        result.push(c);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bold() {
        let result = markdown_to_irc("this is **bold** text");
        assert!(result.contains(BOLD));
        assert!(result.contains("bold"));
        assert!(!result.contains("**"));
    }

    #[test]
    fn test_italic() {
        let result = markdown_to_irc("this is *italic* text");
        assert!(result.contains(ITALIC));
        assert!(result.contains("italic"));
        assert!(!result.contains('*'));
    }

    #[test]
    fn test_code() {
        let result = markdown_to_irc("run `ls -la` command");
        assert_eq!(result, "run ls -la command");
    }

    #[test]
    fn test_mixed() {
        let result = markdown_to_irc("**bold** and *italic*");
        assert!(result.contains(BOLD));
        assert!(result.contains(ITALIC));
    }
}
