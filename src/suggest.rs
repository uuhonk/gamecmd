//! 补全：从一段输入算出"接下来能填什么"。
//!
//! 入口是 [`Completion`]（由 [`CommandRegistry::complete`] 产出）。它同时给出三类
//! 信息：
//!
//! 1. [`Completion::phase`] —— 现在处于命令候选模式、参数模式，还是走不通了；
//! 2. [`Completion::params`] —— 命令的参数名 / 字面量分支提示（"这个命令长什么
//!    样、我填到第几个了"）；
//! 3. [`Completion::candidates`] —— 光标当前这个位置上**能填的值**，已经模糊匹配
//!    过滤并排好序。
//!
//! 排序规则（越靠前越优先）：前缀匹配 > 分数高 > 同类里注册顺序靠前。前缀永远压
//! 过内部匹配，因为"我打了 `pl`"基本就是想补 `player_id`。
//!
//! [`CommandRegistry::complete`]: crate::CommandRegistry::complete

use crate::candidate::{Candidate, ValueHint};
use crate::matcher;
use crate::parse::{ParsedInput, Token};
use crate::registry::CommandRegistry;
use crate::spec::CommandNode;

/// 当前输入处在哪个阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// 还没出现第一个空格：在补命令名。
    Command,
    /// 命令已确定，在补参数。
    Argument,
    /// 命令不认识，或者参数对不上。
    Invalid,
}

/// 一条补全建议是从哪来的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    /// 命令主名。
    Command,
    /// 命令别名。
    Alias,
    /// 字面量分支（`set` / `query`）。
    Literal,
    /// 参数值。
    Value,
}

impl SuggestionKind {
    /// 排序用的优先级，小的在前。
    fn priority(self) -> u8 {
        match self {
            SuggestionKind::Command => 0,
            SuggestionKind::Alias => 1,
            SuggestionKind::Literal => 2,
            SuggestionKind::Value => 3,
        }
    }
}

/// 一条补全建议。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// 候选本身。
    pub candidate: Candidate,
    /// 它是什么（命令 / 别名 / 字面量 / 参数值）。
    pub kind: SuggestionKind,
    /// 匹配分数，越高越靠前。
    pub score: i32,
    /// 命中的字符下标（按 `char` 计）—— 界面可以拿它高亮。
    pub matches: Vec<usize>,
    /// 输入片段是不是这个候选的前缀。
    pub prefix: bool,
}

impl Suggestion {
    /// 补全时会写进输入串的值。
    pub fn value(&self) -> &str {
        &self.candidate.value
    }

    /// 界面显示的文字。
    pub fn display(&self) -> &str {
        self.candidate.display()
    }
}

/// 一个参数槽位（参数或字面量分支）的提示。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamHint {
    /// 参数名（字面量分支就是字面量本身）。
    pub name: String,
    /// 可选参数。
    pub optional: bool,
    /// 变长参数。
    pub variadic: bool,
    /// 这是一个字面量分支吗（不是参数）。
    pub literal: bool,
    /// 值类型提示。
    pub hint: ValueHint,
    /// 候选来源的大类。
    pub source: SourceKind,
    /// 说明文字。
    pub tooltip: Option<String>,
    /// 光标现在停在这个槽位上吗。
    pub current: bool,
}

impl ParamHint {
    /// 用法串里的写法：`<name>` / `[name]` / `<name>...`。
    pub fn slot(&self) -> String {
        let name = if self.variadic {
            format!("{}...", self.name)
        } else {
            self.name.clone()
        };
        if self.optional {
            format!("[{name}]")
        } else {
            format!("<{name}>")
        }
    }
}

/// 参数候选来源的大类（[`crate::ParamSource`] 的简化版，给界面用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// 自由输入，没有候选。
    Free,
    /// 静态候选列表。
    Static,
    /// 枚举候选，附带枚举类型名。
    Enum(String),
    /// 动态候选，附带 source key。
    Dynamic(String),
}

impl SourceKind {
    /// 界面上放在参数名后面的短标签。
    pub fn label(&self) -> String {
        match self {
            SourceKind::Free => "值".to_string(),
            SourceKind::Static => "候选".to_string(),
            SourceKind::Enum(name) => name.clone(),
            SourceKind::Dynamic(source) => format!("{source}..."),
        }
    }
}

/// 补全的行为开关。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompleteOptions {
    /// 最多返回多少条候选。
    pub limit: usize,
    /// 区分大小写（默认 false）。
    pub case_sensitive: bool,
    /// 允许子序列（模糊）匹配（默认 true）。关掉就只做前缀匹配。
    pub fuzzy: bool,
}

impl Default for CompleteOptions {
    fn default() -> Self {
        Self {
            limit: 32,
            case_sensitive: false,
            fuzzy: true,
        }
    }
}

/// 一次补全的完整结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// 补全时的完整输入串。
    pub input: String,
    /// 光标位置（字节偏移）。
    pub cursor: usize,
    /// 当前 token 在输入串里的字节范围 —— 接受一条建议时替换掉它。
    pub span: (usize, usize),
    /// 当前 token 的文本（未加引号）。
    pub fragment: String,
    /// 当前阶段。
    pub phase: Phase,
    /// 命中的命令的**主名**（用别名输入时也是主名）。
    pub command: Option<String>,
    /// 如果用户打的是别名，这里是那个别名。
    pub alias: Option<String>,
    /// 命令说明。
    pub description: Option<String>,
    /// 用法串（`give <player_id> <item> [count]`）。
    pub usage: Option<String>,
    /// 参数槽位提示，按顺序；当前槽位带 `current = true`。
    pub params: Vec<ParamHint>,
    /// 当前正在补第几个参数（下标进 [`Completion::params`]）。
    pub param_index: Option<usize>,
    /// 当前 token 的候选值，已排序。
    pub candidates: Vec<Suggestion>,
    /// 当前槽位 / 命令的说明文字。
    pub tooltip: Option<String>,
    /// 出错说明（可以直接显示给用户）。
    pub error: Option<String>,
}

impl Completion {
    /// 有候选吗。
    pub fn has_candidates(&self) -> bool {
        !self.candidates.is_empty()
    }

    /// 第 `index` 条建议的说明：优先用候选自己的，其次用槽位 / 命令的。
    pub fn tooltip_at(&self, index: usize) -> Option<&str> {
        self.candidates
            .get(index)
            .and_then(|suggestion| suggestion.candidate.tooltip.as_deref())
            .or(self.tooltip.as_deref())
    }

    /// 接受第 `index` 条建议：返回新的输入串和新的光标位置。
    ///
    /// 只替换当前 token，并且（当补全发生在输入末尾时）补一个空格，这样用户可以
    /// 直接接着打下一个参数。值里有空格会自动加引号。
    pub fn apply(&self, index: usize) -> Option<(String, usize)> {
        let suggestion = self.candidates.get(index)?;
        let (start, end) = self.span;
        let value = &suggestion.candidate.value;

        let mut output = String::with_capacity(self.input.len() + value.len() + 4);
        output.push_str(&self.input[..start]);
        let needs_quotes = value.chars().any(char::is_whitespace);
        if needs_quotes {
            output.push('"');
        }
        output.push_str(value);
        if needs_quotes {
            output.push('"');
        }
        let mut cursor = output.len();
        output.push_str(&self.input[end..]);
        if end >= self.input.len() {
            output.push(' ');
            cursor += 1;
        }
        Some((output, cursor))
    }

    /// 当前片段是不是已经**精确**（忽略大小写）等于某条候选。
    pub fn exact_candidate(&self) -> Option<&Suggestion> {
        self.candidates.iter().find(|suggestion| {
            suggestion
                .candidate
                .value
                .eq_ignore_ascii_case(&self.fragment)
        })
    }
}

/// 打分前的一条候选。列表顺序就是注册顺序 —— [`rank`] 用的是稳定排序，所以同分的
/// 候选会保持注册顺序。
struct Pool {
    candidate: Candidate,
    kind: SuggestionKind,
}

/// 核心：算一次补全。由 [`CommandRegistry::complete`] 调用。
///
/// [`CommandRegistry::complete`]: crate::CommandRegistry::complete
pub(crate) fn complete(
    registry: &CommandRegistry,
    input: &str,
    cursor: usize,
    options: &CompleteOptions,
) -> Completion {
    let parsed = ParsedInput::parse(input, cursor);
    let mut completion = Completion {
        input: input.to_string(),
        cursor: parsed.cursor,
        span: (parsed.current.start, parsed.current.end),
        fragment: parsed.current.text.clone(),
        phase: Phase::Command,
        command: None,
        alias: None,
        description: None,
        usage: None,
        params: Vec::new(),
        param_index: None,
        candidates: Vec::new(),
        tooltip: None,
        error: None,
    };

    // (1) 还没出现第一个空格：命令候选模式。
    if parsed.at_command_start() {
        completion.candidates = rank(&completion.fragment, command_pool(registry), options);
        return completion;
    }

    // (2) 第一个 token 是命令名：不在注册表里就没有下文了。
    let head = parsed.head().expect("checked above").text.clone();
    let Some(spec) = registry.find(&head, options.case_sensitive) else {
        completion.phase = Phase::Invalid;
        completion.error = Some(format!("未知命令：{head}"));
        // 顺手把相近的命令名给出来，用户可以直接补全改掉。
        completion.candidates = rank(&head, command_pool(registry), options);
        return completion;
    };

    completion.command = Some(spec.name.clone());
    if !spec.name.eq_ignore_ascii_case(&head) {
        completion.alias = Some(head.clone());
    }
    completion.description = spec.description.clone();
    completion.usage = Some(spec.usage());
    completion.phase = Phase::Argument;

    // (3) 沿着参数树走下去，看已经填好的参数把光标带到了哪个节点。
    let mut nodes: &[CommandNode] = &spec.nodes;
    let mut typed: Vec<&CommandNode> = Vec::new();
    for token in parsed.arguments() {
        match find_child(nodes, token, registry, options) {
            Some((node, issue)) => {
                if issue.is_some() && completion.error.is_none() {
                    completion.error = issue;
                }
                typed.push(node);
                nodes = node.children();
            }
            None => {
                if completion.error.is_none() {
                    completion.error = if nodes.is_empty() {
                        Some(format!(
                            "命令 `{}` 不接受更多参数：{}",
                            spec.name, token.text
                        ))
                    } else {
                        Some(format!("这里不能填 `{}`", token.text))
                    };
                }
                completion.phase = Phase::Invalid;
                nodes = &[];
                break;
            }
        }
    }

    // 参数名提示：已填过的 + 沿第一条分支往后剩下的。
    completion.params = collect_hints(&typed, nodes);
    if nodes.is_empty() {
        completion.param_index = None;
    } else {
        let index = typed.len();
        completion.param_index = Some(index);
        if let Some(hint) = completion.params.get_mut(index) {
            hint.current = true;
            completion.tooltip = hint.tooltip.clone();
        }
    }
    if completion.tooltip.is_none() {
        completion.tooltip = spec.description.clone();
    }

    // (4) 当前 token 的候选。
    completion.candidates = rank(&completion.fragment, node_pool(registry, nodes), options);
    completion
}

/// 命令名 + 别名组成的候选池。
fn command_pool(registry: &CommandRegistry) -> Vec<Pool> {
    let mut pool = Vec::new();
    for spec in registry.commands() {
        pool.push(Pool {
            candidate: Candidate {
                value: spec.name.clone(),
                label: None,
                tooltip: spec.description.clone(),
            },
            kind: SuggestionKind::Command,
        });
        for alias in &spec.aliases {
            pool.push(Pool {
                candidate: Candidate {
                    value: alias.clone(),
                    label: Some(format!("{}（{} 的别名）", alias, spec.name)),
                    tooltip: spec.description.clone(),
                },
                kind: SuggestionKind::Alias,
            });
        }
    }
    pool
}

/// 当前位置上所有节点能提供的候选：字面量分支 + 参数值。
fn node_pool(registry: &CommandRegistry, nodes: &[CommandNode]) -> Vec<Pool> {
    let mut pool = Vec::new();
    for node in nodes {
        match node {
            CommandNode::Literal { name, tooltip, .. } => pool.push(Pool {
                candidate: Candidate {
                    value: name.clone(),
                    label: None,
                    tooltip: tooltip.clone(),
                },
                kind: SuggestionKind::Literal,
            }),
            CommandNode::Argument { spec, .. } => {
                for candidate in registry.param_candidates(spec) {
                    let mut candidate = candidate;
                    if candidate.tooltip.is_none() {
                        candidate.tooltip = spec.tooltip.clone();
                    }
                    pool.push(Pool {
                        candidate,
                        kind: SuggestionKind::Value,
                    });
                }
            }
        }
    }
    pool
}

/// 过滤 + 排序 + 截断。
fn rank(fragment: &str, pool: Vec<Pool>, options: &CompleteOptions) -> Vec<Suggestion> {
    let mut ranked: Vec<Suggestion> = Vec::new();

    for entry in pool {
        let (prefix, score, matches) = if fragment.is_empty() {
            (false, 0, Vec::new())
        } else if options.fuzzy {
            match matcher::fuzzy_match(fragment, &entry.candidate.value, options.case_sensitive) {
                Some(found) => (
                    matcher::is_prefix(fragment, &entry.candidate.value, options.case_sensitive),
                    found.score,
                    found.indices,
                ),
                None => continue,
            }
        } else {
            match matcher::prefix_match(fragment, &entry.candidate.value, options.case_sensitive) {
                Some(found) => (true, found.score, found.indices),
                None => continue,
            }
        };

        ranked.push(Suggestion {
            candidate: entry.candidate,
            kind: entry.kind,
            score,
            matches,
            prefix,
        });
    }

    // 稳定排序：同分时保持注册顺序，所以游戏可以用注册顺序决定展示顺序。
    ranked.sort_by(|a, b| {
        b.prefix
            .cmp(&a.prefix)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| a.kind.priority().cmp(&b.kind.priority()))
    });
    // 同一个值只留第一条（别名可能和主名一样、不同分支可能有同一个字面量）。
    let mut seen: Vec<String> = Vec::new();
    ranked.retain(|suggestion| {
        let key = suggestion.candidate.value.to_ascii_lowercase();
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    ranked.truncate(options.limit);
    ranked
}

/// 在当前位置找一个能吃下 `token` 的子节点。
///
/// 字面量优先（精确对上就用它），否则用第一个参数节点。返回的第二个值是"这个值
/// 有问题吗"——参数值没命中静态候选时不算走不通，只是给个提示，所以还是继续往下
/// 走，让界面能显示后续提示。
fn find_child<'a>(
    nodes: &'a [CommandNode],
    token: &Token,
    registry: &CommandRegistry,
    options: &CompleteOptions,
) -> Option<(&'a CommandNode, Option<String>)> {
    let equal = |node: &CommandNode| match node {
        CommandNode::Literal { name, .. } => {
            if options.case_sensitive {
                name == &token.text
            } else {
                name.eq_ignore_ascii_case(&token.text)
            }
        }
        CommandNode::Argument { .. } => false,
    };
    if let Some(literal) = nodes.iter().find(|node| equal(node)) {
        return Some((literal, None));
    }

    let argument = nodes
        .iter()
        .find(|node| matches!(node, CommandNode::Argument { .. }))?;
    let CommandNode::Argument { spec, .. } = argument else {
        return None;
    };
    let issue = spec
        .parse(&token.text, registry.dynamic(&dynamic_source(spec)))
        .err()
        .map(|error| format!("{}：{error}", spec.name));
    Some((argument, issue))
}

/// 动态候选来源的 key（不是动态参数就是空串）。
fn dynamic_source(spec: &crate::spec::ParamSpec) -> String {
    match &spec.source {
        crate::spec::ParamSource::Dynamic { source } => source.clone(),
        _ => String::new(),
    }
}

/// 已填过的槽位 + 沿第一条分支往后剩下的槽位。
fn collect_hints(typed: &[&CommandNode], nodes: &[CommandNode]) -> Vec<ParamHint> {
    let mut hints: Vec<ParamHint> = typed.iter().map(|node| hint_of(node, false)).collect();
    let mut cursor = nodes.first();
    while let Some(node) = cursor {
        hints.push(hint_of(node, false));
        cursor = node.children().first();
    }
    hints
}

/// 一个节点的提示。
fn hint_of(node: &CommandNode, current: bool) -> ParamHint {
    match node {
        CommandNode::Literal { name, tooltip, .. } => ParamHint {
            name: name.clone(),
            optional: false,
            variadic: false,
            literal: true,
            hint: ValueHint::Any,
            source: SourceKind::Static,
            tooltip: tooltip.clone(),
            current,
        },
        CommandNode::Argument { spec, .. } => ParamHint {
            name: spec.name.clone(),
            optional: spec.optional,
            variadic: spec.variadic,
            literal: false,
            hint: spec.hint,
            source: source_kind(spec),
            tooltip: spec.tooltip.clone(),
            current,
        },
    }
}

/// [`crate::ParamSource`] → [`SourceKind`]。
pub(crate) fn source_kind(spec: &crate::spec::ParamSpec) -> SourceKind {
    match &spec.source {
        crate::spec::ParamSource::Static(_) => SourceKind::Static,
        crate::spec::ParamSource::Enum { name, .. } => SourceKind::Enum(name.clone()),
        crate::spec::ParamSource::Dynamic { source } => SourceKind::Dynamic(source.clone()),
        crate::spec::ParamSource::Free { .. } => SourceKind::Free,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::CommandRegistry;
    use crate::spec::{CommandSpec, ParamSpec};

    fn registry() -> CommandRegistry {
        let mut registry = CommandRegistry::new();
        registry.register(
            CommandSpec::new("give")
                .describe("给一名玩家一件物品")
                .alias("g")
                .param(ParamSpec::dynamic("player_id", "players"))
                .param(ParamSpec::choices("item", ["sword", "cash"]))
                .param(ParamSpec::free("count", ValueHint::Integer).optional()),
        );
        registry.set_dynamic("players", ["p1", "p2"]);
        registry.register(
            CommandSpec::new("time")
                .branch(
                    crate::spec::CommandNode::literal("set")
                        .then_param(ParamSpec::choices("value", ["day", "night"])),
                )
                .branch(crate::spec::CommandNode::literal("query")),
        );
        registry.register(CommandSpec::new("help").describe("列出命令"));
        registry
    }

    #[test]
    fn command_mode_ranks_prefix_matches_first() {
        let registry = registry();
        let completion = registry.complete("g", 1);
        assert_eq!(completion.phase, Phase::Command);
        assert!(completion.command.is_none());
        let values: Vec<&str> = completion
            .candidates
            .iter()
            .map(|suggestion| suggestion.value())
            .collect();
        assert_eq!(values[0], "give");
        assert!(values.contains(&"g")); // 别名
        assert!(completion.candidates[0].prefix);
    }

    #[test]
    fn an_empty_input_lists_every_command() {
        let registry = registry();
        let completion = registry.complete("", 0);
        assert_eq!(completion.phase, Phase::Command);
        // give + 它的别名 g + time + help
        assert_eq!(completion.candidates.len(), 4);
        assert_eq!(completion.span, (0, 0));
    }

    #[test]
    fn a_space_after_the_command_yields_params_and_the_first_param_values() {
        let registry = registry();
        let completion = registry.complete("give ", 5);
        assert_eq!(completion.phase, Phase::Argument);
        assert_eq!(completion.command.as_deref(), Some("give"));
        assert_eq!(
            completion.usage.as_deref(),
            Some("give <player_id> <item> [count]")
        );
        assert_eq!(
            completion
                .params
                .iter()
                .map(|hint| hint.name.as_str())
                .collect::<Vec<_>>(),
            ["player_id", "item", "count"]
        );
        assert_eq!(completion.param_index, Some(0));
        assert!(completion.params[0].current);
        // 第一个参数是动态候选：没有快照就是空，有快照就列出来。
        let values: Vec<&str> = completion
            .candidates
            .iter()
            .map(|suggestion| suggestion.value())
            .collect();
        assert_eq!(values, ["p1", "p2"]);
        assert_eq!(completion.span, (5, 5));
    }

    #[test]
    fn candidates_narrow_as_the_fragment_grows() {
        let registry = registry();
        let completion = registry.complete("give p", 6);
        assert_eq!(
            completion
                .candidates
                .iter()
                .map(|suggestion| suggestion.value())
                .collect::<Vec<_>>(),
            ["p1", "p2"]
        );
        let completion = registry.complete("give p2 ", 8);
        assert_eq!(completion.param_index, Some(1));
        assert_eq!(
            completion
                .candidates
                .iter()
                .map(|suggestion| suggestion.value())
                .collect::<Vec<_>>(),
            ["sword", "cash"]
        );
    }

    #[test]
    fn a_branch_narrows_the_following_parameters() {
        let registry = registry();
        let completion = registry.complete("time ", 5);
        // 两个字面量分支都在候选里。
        assert_eq!(
            completion
                .candidates
                .iter()
                .map(|suggestion| suggestion.value())
                .collect::<Vec<_>>(),
            ["set", "query"]
        );
        // 走到 set 分支之后，出现的才是 value 的候选。
        let completion = registry.complete("time set ", 9);
        assert_eq!(completion.param_index, Some(1));
        assert_eq!(
            completion
                .candidates
                .iter()
                .map(|suggestion| suggestion.value())
                .collect::<Vec<_>>(),
            ["day", "night"]
        );
        // query 没有后续参数。
        let completion = registry.complete("time query ", 11);
        assert!(completion.params.iter().all(|hint| hint.name != "value"));
        assert!(completion.candidates.is_empty());
    }

    #[test]
    fn a_finished_branch_reports_no_more_slots() {
        let registry = registry();
        // 光标还停在分支名上时，补的是分支（第一个槽位）。
        let completion = registry.complete("time query", 10);
        assert_eq!(completion.phase, Phase::Argument);
        assert_eq!(completion.param_index, Some(0));

        // 分支名后面跟了空格：命令已经完整，没有下一个槽位了。
        let completion = registry.complete("time query ", 11);
        assert_eq!(completion.phase, Phase::Argument);
        assert_eq!(completion.param_index, None);
        assert!(completion.candidates.is_empty());
        assert_eq!(
            completion
                .params
                .iter()
                .map(|hint| hint.name.as_str())
                .collect::<Vec<_>>(),
            ["query"]
        );
    }

    #[test]
    fn an_unknown_command_reports_an_error_but_still_suggests_commands() {
        let registry = registry();
        let completion = registry.complete("giv ", 4);
        assert_eq!(completion.phase, Phase::Invalid);
        assert!(completion.error.as_deref().unwrap().contains("giv"));
        assert!(completion
            .candidates
            .iter()
            .any(|suggestion| suggestion.value() == "give"));
    }

    #[test]
    fn an_invalid_value_is_reported_but_the_path_continues() {
        let registry = registry();
        let completion = registry.complete("give p1 nope ", 13);
        assert!(completion.error.as_deref().unwrap().contains("item"));
        // 仍然知道下一个参数是 count。
        assert_eq!(completion.param_index, Some(2));
    }

    #[test]
    fn too_many_arguments_is_reported() {
        let registry = registry();
        let completion = registry.complete("help now ", 9);
        assert_eq!(completion.phase, Phase::Invalid);
        assert!(completion.error.is_some());
    }

    #[test]
    fn apply_replaces_the_current_token_and_appends_a_space() {
        let registry = registry();
        let completion = registry.complete("give p1 sw", 10);
        let (input, cursor) = completion.apply(0).unwrap();
        assert_eq!(input, "give p1 sword ");
        assert_eq!(cursor, input.len());
    }

    #[test]
    fn apply_keeps_the_suffix_and_quotes_values_with_spaces() {
        let mut registry = CommandRegistry::new();
        registry.register(
            CommandSpec::new("say").param(ParamSpec::choices("word", ["hello world", "hi"])),
        );
        let completion = registry.complete("say hel", 7);
        let (input, cursor) = completion.apply(0).unwrap();
        assert_eq!(input, "say \"hello world\" ");
        assert_eq!(cursor, input.len());

        // 光标在中间时不动后面的内容。
        let completion = registry.complete("say hel now", 7);
        let (input, _) = completion.apply(0).unwrap();
        assert_eq!(input, "say \"hello world\" now");
    }

    #[test]
    fn options_can_turn_fuzzy_matching_off() {
        let registry = registry();
        let strict = CompleteOptions {
            fuzzy: false,
            ..CompleteOptions::default()
        };
        // `gv` 是 `give` 的子序列，但不是前缀。
        assert!(registry
            .complete_with("gv", 2, &strict)
            .candidates
            .is_empty());
        assert!(!registry.complete("gv", 2).candidates.is_empty());
    }

    #[test]
    fn the_limit_truncates_the_candidate_list() {
        let registry = registry();
        let limited = CompleteOptions {
            limit: 1,
            ..CompleteOptions::default()
        };
        assert_eq!(registry.complete_with("", 0, &limited).candidates.len(), 1);
    }

    #[test]
    fn matches_are_reported_for_highlighting() {
        let registry = registry();
        let completion = registry.complete("gv", 2);
        let suggestion = completion
            .candidates
            .iter()
            .find(|suggestion| suggestion.value() == "give")
            .unwrap();
        assert_eq!(suggestion.matches, vec![0, 2]);
    }
}
