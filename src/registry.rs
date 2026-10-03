//! 命令注册表：命令表 + 动态候选快照 + 补全 / 解析 / 执行入口。

use std::collections::BTreeMap;

use crate::candidate::{Candidate, Value};
use crate::parse::tokenize;
use crate::spec::{CommandNode, CommandSpec, ParamSource, ParamSpec};
use crate::suggest::{self, CompleteOptions, Completion};

/// **纯命令**的执行体：只有解析好的调用，拿不到外部状态（世界、资源、渲染器……）。
///
/// `#[command]` 标记的函数如果没有上下文参数，就会生成一个这样的函数并挂到
/// [`CommandSpec::run`] 上。需要外部状态的命令（`fn f(state: &mut GameState, ...)`）
/// 生成的是另一个签名，由调用方自己接执行 —— 见 crate 文档里的"带上下文的命令"。
pub type CommandFn = fn(&Invocation) -> Result<Option<String>, CommandError>;

/// 命令注册表。
///
/// 它持有一切"命令长什么样"的数据：
///
/// - [`CommandRegistry::register`] 进来的命令表（[`CommandSpec`]）；
/// - **动态候选快照**：`"players" -> [p1, p2]` 这种"游戏当前状态"的候选值。注册表
///   不知道这些值从哪来（ECS？网络？写死的？），游戏在数据变化时调用
///   [`CommandRegistry::set_dynamic`] 推一次就够了。
///
/// 补全（[`CommandRegistry::complete`]）和解析（[`CommandRegistry::resolve`]）都在
/// 这里。注册表本身是可克隆的纯数据，可以整个塞进调用方的全局状态里。
///
/// # 例子
///
/// ```
/// use gamecmd::{CommandRegistry, CommandSpec, ParamSpec, ValueHint};
///
/// let mut registry = CommandRegistry::new();
/// registry.register(CommandSpec::new("time").branch(
///     gamecmd::CommandNode::literal("set").then_param(ParamSpec::choices("value", ["day", "night"])),
/// ));
/// registry.register(CommandSpec::new("say").param(ParamSpec::free("text", ValueHint::QuotedText)));
///
/// // 命令名不区分大小写。
/// assert!(registry.find("TIME", false).is_some());
///
/// // 动态候选：先推快照，再补全。
/// registry.set_dynamic("players", ["p1", "p2"]);
/// assert_eq!(registry.dynamic("players").len(), 2);
/// assert!(registry.dynamic("unknown").is_empty());
/// ```
#[derive(Clone, Debug, Default)]
pub struct CommandRegistry {
    commands: Vec<CommandSpec>,
    dynamic: BTreeMap<String, Vec<Candidate>>,
}

impl CommandRegistry {
    /// 一个空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    // --- 注册 -----------------------------------------------------------------

    /// 注册（或按名字替换）一个命令。返回 `&mut Self`，方便连着写。
    pub fn register(&mut self, command: CommandSpec) -> &mut Self {
        match self
            .commands
            .iter_mut()
            .find(|existing| existing.name == command.name)
        {
            Some(existing) => *existing = command,
            None => self.commands.push(command),
        }
        self
    }

    /// 按主名移除一个命令。移除成功返回 `true`。
    pub fn unregister(&mut self, name: &str) -> bool {
        let before = self.commands.len();
        self.commands.retain(|command| command.name != name);
        self.commands.len() != before
    }

    /// 移除全部命令（保留动态候选快照）。
    pub fn clear(&mut self) {
        self.commands.clear();
    }

    /// 全部命令，按注册顺序。
    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    /// 一个命令都没有吗。
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// 命令数量。
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// 按主名或别名找命令（默认忽略大小写）。
    pub fn get(&self, name: &str) -> Option<&CommandSpec> {
        self.find(name, false)
    }

    /// 按主名或别名找命令。
    pub fn find(&self, name: &str, case_sensitive: bool) -> Option<&CommandSpec> {
        self.commands
            .iter()
            .find(|command| command.matches_name(name, case_sensitive))
    }

    // --- 动态候选 -------------------------------------------------------------

    /// 推一份动态候选快照（覆盖同一个 `source` 上的旧值）。
    ///
    /// 游戏在数据变化时调用它，比如玩家列表变了、物品表热重载完了。
    pub fn set_dynamic(
        &mut self,
        source: impl Into<String>,
        candidates: impl IntoIterator<Item = impl Into<Candidate>>,
    ) -> &mut Self {
        let candidates = candidates.into_iter().map(Into::into).collect();
        self.dynamic.insert(source.into(), candidates);
        self
    }

    /// 清掉一个动态候选来源（之后它会被当成"还没有数据"）。
    pub fn clear_dynamic(&mut self, source: &str) {
        self.dynamic.remove(source);
    }

    /// 清掉全部动态候选快照。
    pub fn clear_all_dynamic(&mut self) {
        self.dynamic.clear();
    }

    /// 一个动态候选来源的当前快照；没有就是空。
    pub fn dynamic(&self, source: &str) -> &[Candidate] {
        self.dynamic.get(source).map(Vec::as_slice).unwrap_or(&[])
    }

    /// 一个参数当前**实际可选**的值：静态 / 枚举参数是它自己的列表，动态参数是
    /// 快照，自由参数没有候选。
    pub fn param_candidates(&self, spec: &ParamSpec) -> Vec<Candidate> {
        match &spec.source {
            ParamSource::Dynamic { source } => self.dynamic(source).to_vec(),
            other => other.own_candidates().to_vec(),
        }
    }

    // --- 补全 / 解析 / 执行 -----------------------------------------------------

    /// 算一次补全（默认选项，见 [`CompleteOptions::default`]）。
    pub fn complete(&self, input: &str, cursor: usize) -> Completion {
        self.complete_with(input, cursor, &CompleteOptions::default())
    }

    /// 算一次补全。
    ///
    /// 这是整个 crate 的主入口：界面把输入串和光标位置喂进来，拿回的
    /// [`Completion`] 里有阶段、参数提示和候选列表。
    pub fn complete_with(
        &self,
        input: &str,
        cursor: usize,
        options: &CompleteOptions,
    ) -> Completion {
        suggest::complete(self, input, cursor, options)
    }

    /// 解析一条输入并执行它（只对挂了 [`CommandSpec::run`] 的纯命令有效）。
    ///
    /// 需要外部状态的命令请用 [`CommandRegistry::resolve`] 自己解析，再把
    /// [`Invocation`] 交给调用方处理。
    pub fn execute(&self, input: &str) -> Result<Option<String>, CommandError> {
        let invocation = self.resolve(input)?;
        self.execute_invocation(&invocation)
    }

    /// 执行一条已经解析好的调用。
    pub fn execute_invocation(
        &self,
        invocation: &Invocation,
    ) -> Result<Option<String>, CommandError> {
        let spec = self
            .find(&invocation.command, false)
            .ok_or_else(|| CommandError::UnknownCommand(invocation.command.clone()))?;
        match spec.run {
            Some(run) => run(invocation),
            None => Err(CommandError::NoHandler(invocation.command.clone())),
        }
    }

    /// 把一条**完整**的输入解析成结构化结果。
    ///
    /// 和补全共用同一棵参数树：走不通的地方返回 [`CommandError`]。
    pub fn resolve(&self, input: &str) -> Result<Invocation, CommandError> {
        let tokens = tokenize(input);
        let Some(head) = tokens.first() else {
            return Err(CommandError::Empty);
        };
        let spec = self
            .find(&head.text, false)
            .ok_or_else(|| CommandError::UnknownCommand(head.text.clone()))?;
        let alias = if spec.name == head.text {
            None
        } else {
            Some(head.text.clone())
        };

        let mut nodes: &[CommandNode] = &spec.nodes;
        let mut literals: Vec<String> = Vec::new();
        let mut args: Vec<InvocationArg> = Vec::new();
        let mut arg_index = 0usize;

        for token in &tokens[1..] {
            // 字面量分支优先（和补全里的规则一致）。
            if let Some(literal) = nodes.iter().find(|node| {
                matches!(node, CommandNode::Literal { name, .. } if name.eq_ignore_ascii_case(&token.text))
            }) {
                literals.push(literal.name().to_string());
                nodes = literal.children();
                continue;
            }

            let Some(argument) = nodes
                .iter()
                .find(|node| matches!(node, CommandNode::Argument { .. }))
            else {
                return Err(if nodes.is_empty() {
                    CommandError::TooManyArguments {
                        command: spec.name.clone(),
                        extra: token.text.clone(),
                    }
                } else {
                    CommandError::UnexpectedArgument {
                        command: spec.name.clone(),
                        value: token.text.clone(),
                        expected: nodes.iter().map(|node| node.name().to_string()).collect(),
                    }
                });
            };

            let CommandNode::Argument {
                spec: param,
                children,
            } = argument
            else {
                unreachable!("filtered for Argument above");
            };
            let dynamic = match &param.source {
                ParamSource::Dynamic { source } => self.dynamic(source),
                _ => &[],
            };
            let value =
                param
                    .parse(&token.text, dynamic)
                    .map_err(|error| CommandError::InvalidValue {
                        command: spec.name.clone(),
                        param: param.name.clone(),
                        value: token.text.clone(),
                        reason: error.to_string(),
                    })?;
            args.push(InvocationArg {
                name: param.name.clone(),
                value,
                raw: token.text.clone(),
                index: arg_index,
            });
            // 变长参数（且后面没别的参数）可以一直吃同一个位置。
            let repeats = param.variadic && children.is_empty();
            if !repeats {
                nodes = children;
                arg_index += 1;
            }
        }

        // 还缺必填参数吗（可选 / 变长参数不算缺）。
        if let Some(node) = nodes.first() {
            let missing = match node {
                CommandNode::Literal { name, .. } => Some(name.clone()),
                CommandNode::Argument { spec: param, .. } => {
                    (!param.optional && !param.variadic).then(|| param.name.clone())
                }
            };
            if let Some(param) = missing {
                return Err(CommandError::MissingArgument {
                    command: spec.name.clone(),
                    param,
                });
            }
        }

        Ok(Invocation {
            command: spec.name.clone(),
            alias,
            literals,
            args,
        })
    }
}

/// 一条解析成功的命令。
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    /// 命令主名（用别名输入时也是主名）。
    pub command: String,
    /// 如果用户打的是别名，这里是那个别名。
    pub alias: Option<String>,
    /// 走过的字面量分支（比如 `/time set` 的 `set`）。
    pub literals: Vec<String>,
    /// 参数值，按输入顺序。
    pub args: Vec<InvocationArg>,
}

impl Invocation {
    /// 按参数名取第一个值。
    pub fn arg(&self, name: &str) -> Option<&InvocationArg> {
        self.args.iter().find(|arg| arg.name == name)
    }

    /// 按参数名取值。
    pub fn value(&self, name: &str) -> Option<&Value> {
        self.arg(name).map(|arg| &arg.value)
    }
}

/// 一个解析出来的参数。
#[derive(Clone, Debug, PartialEq)]
pub struct InvocationArg {
    /// 参数名。
    pub name: String,
    /// 解析后的值。
    pub value: Value,
    /// 输入串里的原文。
    pub raw: String,
    /// 参数在签名里的下标（变长参数的重复项共用同一个下标）。
    pub index: usize,
}

/// 解析一条命令失败的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// 输入是空的。
    Empty,
    /// 命令不在注册表里。
    UnknownCommand(String),
    /// 参数值不合法。
    InvalidValue {
        /// 命令名。
        command: String,
        /// 参数名。
        param: String,
        /// 输入的值。
        value: String,
        /// 为什么不行。
        reason: String,
    },
    /// 这里不接受这个参数（比如 `/time query day` 里的 `day`）。
    UnexpectedArgument {
        /// 命令名。
        command: String,
        /// 输入的值。
        value: String,
        /// 这里本来可以填什么。
        expected: Vec<String>,
    },
    /// 参数给多了。
    TooManyArguments {
        /// 命令名。
        command: String,
        /// 多出来的那个。
        extra: String,
    },
    /// 必填参数没给。
    MissingArgument {
        /// 命令名。
        command: String,
        /// 缺的参数名。
        param: String,
    },
    /// 命令解析没问题，但宿主没给它接执行体。
    NoHandler(String),
    /// 执行体自己报的错（`Result<_, E>` 里的 `E`）。
    Execution(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::Empty => f.write_str("命令是空的"),
            CommandError::UnknownCommand(name) => write!(f, "未知命令：{name}"),
            CommandError::InvalidValue {
                command,
                param,
                value,
                reason,
            } => write!(f, "{command} 的参数 `{param}` 无效（{value}）：{reason}"),
            CommandError::UnexpectedArgument {
                command,
                value,
                expected,
            } => write!(
                f,
                "{command} 在这里不能填 `{value}`（可以填：{}）",
                expected.join(" / ")
            ),
            CommandError::TooManyArguments { command, extra } => {
                write!(f, "{command} 不接受更多参数：{extra}")
            }
            CommandError::MissingArgument { command, param } => {
                write!(f, "{command} 缺少参数 `{param}`")
            }
            CommandError::NoHandler(command) => write!(f, "{command} 还没有接上执行体"),
            CommandError::Execution(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::ValueHint;

    fn registry() -> CommandRegistry {
        let mut registry = CommandRegistry::new();
        registry.register(
            CommandSpec::new("give")
                .alias("g")
                .param(ParamSpec::dynamic("player_id", "players"))
                .param(ParamSpec::choices("item", ["sword", "cash"]))
                .param(ParamSpec::free("count", ValueHint::Integer).optional()),
        );
        registry.set_dynamic("players", ["p1", "p2"]);
        registry.register(
            CommandSpec::new("time")
                .branch(
                    CommandNode::literal("set")
                        .then_param(ParamSpec::choices("value", ["day", "night"])),
                )
                .branch(CommandNode::literal("query")),
        );
        registry.register(
            CommandSpec::new("say").param(ParamSpec::free("text", ValueHint::QuotedText)),
        );
        registry.register(
            CommandSpec::new("mark").param(ParamSpec::dynamic("point", "points").variadic()),
        );
        registry
    }

    #[test]
    fn registering_the_same_name_replaces_the_command() {
        let mut registry = CommandRegistry::new();
        registry.register(CommandSpec::new("a").describe("first"));
        registry.register(CommandSpec::new("a").describe("second"));
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("a").unwrap().description.as_deref(),
            Some("second")
        );
    }

    #[test]
    fn lookups_cover_aliases_and_ignore_case() {
        let registry = registry();
        assert_eq!(registry.get("GIVE").unwrap().name, "give");
        assert_eq!(registry.get("g").unwrap().name, "give");
        assert!(registry.get("nope").is_none());
        assert!(!registry.is_empty());
    }

    #[test]
    fn dynamic_snapshots_are_read_back_and_replaced() {
        let mut registry = registry();
        assert_eq!(registry.dynamic("players").len(), 2);
        registry.set_dynamic("players", ["only"]);
        assert_eq!(registry.dynamic("players")[0].value, "only");
        registry.clear_dynamic("players");
        assert!(registry.dynamic("players").is_empty());
        registry.clear_all_dynamic();
        assert!(registry.dynamic("players").is_empty());
    }

    #[test]
    fn param_candidates_come_from_the_right_place() {
        let registry = registry();
        let spec = registry.get("give").unwrap();
        let CommandNode::Argument { spec: player, .. } = &spec.nodes[0] else {
            panic!("first node should be an argument");
        };
        assert_eq!(registry.param_candidates(player).len(), 2);

        let mut free = ParamSpec::free("text", ValueHint::Text);
        assert!(registry.param_candidates(&free).is_empty());
        // 静态 / 枚举参数不碰快照。
        free = ParamSpec::choices("item", ["sword"]);
        assert_eq!(registry.param_candidates(&free).len(), 1);
    }

    #[test]
    fn resolving_a_linear_command_returns_named_values() {
        let registry = registry();
        let invocation = registry.resolve("give p1 sword 3").unwrap();
        assert_eq!(invocation.command, "give");
        assert!(invocation.alias.is_none());
        assert_eq!(invocation.args.len(), 3);
        assert_eq!(
            invocation.arg("player_id").unwrap().value,
            Value::Text("p1".into())
        );
        assert_eq!(invocation.value("item"), Some(&Value::Text("sword".into())));
        assert_eq!(invocation.value("count"), Some(&Value::Integer(3)));
        assert_eq!(invocation.arg("count").unwrap().index, 2);
    }

    #[test]
    fn resolving_accepts_aliases_and_optional_arguments() {
        let registry = registry();
        let invocation = registry.resolve("g p2 CASH").unwrap();
        assert_eq!(invocation.command, "give");
        assert_eq!(invocation.alias.as_deref(), Some("g"));
        // 候选值会规范成注册时写的写法。
        assert_eq!(invocation.value("item"), Some(&Value::Text("cash".into())));
        assert!(invocation.arg("count").is_none());
    }

    #[test]
    fn resolving_quoted_arguments_keeps_the_spaces() {
        let registry = registry();
        let invocation = registry.resolve(r#"say "hello there""#).unwrap();
        assert_eq!(
            invocation.value("text"),
            Some(&Value::Text("hello there".into()))
        );
    }

    #[test]
    fn resolving_branches_records_the_literal_path() {
        let registry = registry();
        let invocation = registry.resolve("time set day").unwrap();
        assert_eq!(invocation.literals, ["set"]);
        assert_eq!(invocation.value("value"), Some(&Value::Text("day".into())));

        let invocation = registry.resolve("time query").unwrap();
        assert_eq!(invocation.literals, ["query"]);
        assert!(invocation.args.is_empty());
    }

    #[test]
    fn variadic_arguments_repeat_at_the_same_index() {
        let mut registry = registry();
        registry.set_dynamic("points", ["a", "b", "c"]);
        let invocation = registry.resolve("mark a b c").unwrap();
        assert_eq!(invocation.args.len(), 3);
        assert!(invocation.args.iter().all(|arg| arg.index == 0));
    }

    #[test]
    fn errors_are_specific() {
        let registry = registry();
        assert_eq!(registry.resolve("   "), Err(CommandError::Empty));
        assert_eq!(
            registry.resolve("nope"),
            Err(CommandError::UnknownCommand("nope".into()))
        );
        assert!(matches!(
            registry.resolve("give p1 nope"),
            Err(CommandError::InvalidValue { .. })
        ));
        assert!(matches!(
            registry.resolve("give p1"),
            Err(CommandError::MissingArgument { .. })
        ));
        assert!(matches!(
            registry.resolve("give p1 sword 1 2"),
            Err(CommandError::TooManyArguments { .. })
        ));
        assert!(matches!(
            registry.resolve("time nope"),
            Err(CommandError::UnexpectedArgument { .. })
        ));
        // query 是叶子，后面再来一个 token 就是"给多了"。
        assert!(matches!(
            registry.resolve("time query day"),
            Err(CommandError::TooManyArguments { .. })
        ));
    }

    #[test]
    fn a_failed_value_parse_says_which_param_it_was() {
        let registry = registry();
        let error = registry.resolve("give p1 sword x").unwrap_err();
        let text = error.to_string();
        assert!(text.contains("count"), "{text}");
        assert!(text.contains('x'), "{text}");
    }

    #[test]
    fn execute_runs_pure_commands_and_reports_missing_handlers() {
        let mut registry = registry();
        let mut say = registry.get("say").unwrap().clone();
        say.run = Some(|invocation| {
            let text = invocation
                .value("text")
                .map(|value| value.to_string())
                .unwrap_or_default();
            Ok(Some(format!("said {text}")))
        });
        registry.register(say);

        assert_eq!(registry.execute("say hi").unwrap(), Some("said hi".into()));
        assert!(matches!(
            registry.execute("give p1 sword"),
            Err(CommandError::NoHandler(_))
        ));
        // 解析失败时执行体根本不会被调用。
        assert!(matches!(
            registry.execute("say"),
            Err(CommandError::MissingArgument { .. })
        ));
    }
}
