//! 输入串的切分：按空白切 token，支持双引号包住的带空格值，并记住光标。

/// 一个 token。
///
/// `raw` 是输入串里的原文（**含**引号），`text` 是去掉引号之后的值 —— 补全和解析
/// 都用 `text`。`start..end` 是它在输入串里的字节范围，补全时用它做替换。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// 去掉引号的文本。
    pub text: String,
    /// 输入串里的原文（含引号）。
    pub raw: String,
    /// 在输入串里的起始字节偏移。
    pub start: usize,
    /// 在输入串里的结束字节偏移（不含）。
    pub end: usize,
    /// 这个 token 里有引号吗。
    pub quoted: bool,
}

impl Token {
    /// 一个空 token（光标前刚打了一个空格时就是它）。
    pub fn empty(at: usize) -> Self {
        Self {
            text: String::new(),
            raw: String::new(),
            start: at,
            end: at,
            quoted: false,
        }
    }

    /// 这个 token 是空的吗。
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// 切分结果。
///
/// 关键区别是 [`ParsedInput::tokens`] 和 [`ParsedInput::current`]：
///
/// ```text
/// "give player1 pl"
///  └──── tokens ────┘└current┘   // 光标在 `pl` 里
/// "give player1 "
///  └─── tokens ───┘└current┘     // current 是空 token：下一个参数
/// ```
///
/// **光标之后的内容会被忽略**（补全只看光标前），所以 `tokens` 里全是"已经敲定"
/// 的 token，`current` 是正在编辑的那一个。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedInput {
    /// 光标之前、已经被空白分隔开的完整 token。
    pub tokens: Vec<Token>,
    /// 光标所在的 token；空 token 表示光标前刚打了一个空格。
    pub current: Token,
    /// 光标位置（字节偏移，已经贴在字符边界上）。
    pub cursor: usize,
}

impl ParsedInput {
    /// 按 `cursor` 切分 `input`。`cursor` 超出范围或不在字符边界上都会被夹到最近的
    /// 合法位置。
    pub fn parse(input: &str, cursor: usize) -> Self {
        let cursor = clamp_to_char_boundary(input, cursor);
        let before = &input[..cursor];

        let mut tokens = Vec::new();
        let mut text = String::new();
        let mut start: Option<usize> = None;
        let mut quoted = false;
        let mut in_quotes = false;

        for (index, character) in before.char_indices() {
            if character == '"' {
                // 引号本身不进 `text`，只标记这个 token 是引用过的。
                start.get_or_insert(index);
                quoted = true;
                in_quotes = !in_quotes;
                continue;
            }
            if character.is_whitespace() && !in_quotes {
                if let Some(token_start) = start.take() {
                    tokens.push(Token {
                        text: std::mem::take(&mut text),
                        raw: before[token_start..index].to_string(),
                        start: token_start,
                        end: index,
                        quoted,
                    });
                    quoted = false;
                }
                continue;
            }
            start.get_or_insert(index);
            text.push(character);
        }

        let current = match start {
            Some(token_start) => Token {
                text,
                raw: before[token_start..].to_string(),
                start: token_start,
                end: cursor,
                quoted,
            },
            None => Token::empty(cursor),
        };

        Self {
            tokens,
            current,
            cursor,
        }
    }

    /// 光标在第一个 token 里吗（还没出现第一个空格 → 命令候选模式）。
    pub fn at_command_start(&self) -> bool {
        self.tokens.is_empty()
    }

    /// 已经敲定的第一个 token（命令名），如果存在。
    pub fn head(&self) -> Option<&Token> {
        self.tokens.first()
    }

    /// 第一个 token 之后的、已经敲定的 token（也就是已经填好的参数）。
    pub fn arguments(&self) -> &[Token] {
        match self.tokens.len() {
            0 => &[],
            _ => &self.tokens[1..],
        }
    }
}

/// 把光标夹进 `input` 并且贴到一个字符边界上。
fn clamp_to_char_boundary(input: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(input.len());
    while cursor > 0 && !input.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

/// 按空白把整个串切成 token（引号内的空白不切分）。补全用
/// [`ParsedInput::parse`]，这个函数给"完整解析一条命令"和测试用。
pub fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut start: Option<usize> = None;
    let mut quoted = false;
    let mut in_quotes = false;

    for (index, character) in input.char_indices() {
        if character == '"' {
            start.get_or_insert(index);
            quoted = true;
            in_quotes = !in_quotes;
            continue;
        }
        if character.is_whitespace() && !in_quotes {
            if let Some(token_start) = start.take() {
                tokens.push(Token {
                    text: std::mem::take(&mut text),
                    raw: input[token_start..index].to_string(),
                    start: token_start,
                    end: index,
                    quoted,
                });
                quoted = false;
            }
            continue;
        }
        start.get_or_insert(index);
        text.push(character);
    }
    if let Some(token_start) = start {
        tokens.push(Token {
            text,
            raw: input[token_start..].to_string(),
            start: token_start,
            end: input.len(),
            quoted,
        });
    }
    tokens
}

/// 按 `cursor` 切分 `input`。
pub fn parse(input: &str, cursor: usize) -> ParsedInput {
    ParsedInput::parse(input, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(tokens: &[Token]) -> Vec<&str> {
        tokens.iter().map(|token| token.text.as_str()).collect()
    }

    #[test]
    fn tokens_split_on_whitespace() {
        // 结尾的空格把最后一个 token 也"敲定"了。
        let parsed = parse("give p1 sword 3 ", 16);
        assert_eq!(texts(&parsed.tokens), ["give", "p1", "sword", "3"]);
        assert!(parsed.current.is_empty());
        assert_eq!(parsed.current.start, 16);
    }

    #[test]
    fn the_last_token_is_the_one_being_edited() {
        let parsed = parse("give p1 sword 3", 15);
        assert_eq!(texts(&parsed.tokens), ["give", "p1", "sword"]);
        assert_eq!(parsed.current.text, "3");
        assert_eq!((parsed.current.start, parsed.current.end), (14, 15));
    }

    #[test]
    fn a_trailing_space_starts_an_empty_token() {
        let parsed = parse("give ", 5);
        assert_eq!(texts(&parsed.tokens), ["give"]);
        assert!(parsed.current.is_empty());
        assert_eq!(parsed.current.start, 5);
        assert!(!parsed.at_command_start());
    }

    #[test]
    fn the_first_token_alone_is_command_mode() {
        let parsed = parse("gi", 2);
        assert!(parsed.at_command_start());
        assert_eq!(parsed.tokens.len(), 0);
        assert_eq!(parsed.current.text, "gi");
        assert_eq!((parsed.current.start, parsed.current.end), (0, 2));
    }

    #[test]
    fn quotes_group_whitespace_into_one_token() {
        let tokens = tokenize(r#"give "player one" sword"#);
        assert_eq!(texts(&tokens), ["give", "player one", "sword"]);
        assert!(tokens[1].quoted);
        assert_eq!(tokens[1].raw, r#""player one""#);
        assert!(!tokens[2].quoted);
    }

    #[test]
    fn an_unterminated_quote_stays_in_the_current_token() {
        let parsed = parse(r#"give "player one"#, 16);
        assert_eq!(texts(&parsed.tokens), ["give"]);
        assert_eq!(parsed.current.text, "player one");
        assert!(parsed.current.quoted);
    }

    #[test]
    fn an_empty_quoted_token_survives() {
        let tokens = tokenize(r#"give "" sword"#);
        assert_eq!(texts(&tokens), ["give", "", "sword"]);
        assert!(tokens[1].quoted);
    }

    #[test]
    fn content_after_the_cursor_is_ignored() {
        let parsed = parse("give player1 sword", 11);
        assert_eq!(texts(&parsed.tokens), ["give"]);
        assert_eq!(parsed.current.text, "player");
    }

    #[test]
    fn the_cursor_is_clamped_into_the_string_and_onto_a_char_boundary() {
        let parsed = parse("玩家列表", 100);
        assert_eq!(parsed.cursor, "玩家列表".len());
        // 落在多字节字符中间时回退到字符边界。
        let parsed = parse("玩家列表", 1);
        assert_eq!(parsed.cursor, 0);
    }

    #[test]
    fn arguments_exclude_the_command_name() {
        let parsed = parse("give p1 sword ", 14);
        assert_eq!(parsed.head().unwrap().text, "give");
        assert_eq!(texts(parsed.arguments()), ["p1", "sword"]);
    }
}
