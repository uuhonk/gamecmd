//! 命令的形状：命令表（[`CommandSpec`]）、参数（[`ParamSpec`]）和参数树
//! （[`CommandNode`]）。

use crate::candidate::{Candidate, Value, ValueHint};
use crate::registry::CommandFn;

/// 一个参数的候选值来源。
///
/// 这就是需求里说的"静态 / 动态 / 枚举"三种注册方式：
///
/// - [`ParamSource::Static`] —— 注册时写死一个列表；
/// - [`ParamSource::Enum`] —— 一个枚举类型的成员（语义上还是列表，只是额外记住
///   类型名，界面可以显示成 `GameMode` 而不是一坨值）；
/// - [`ParamSource::Dynamic`] —— 只登记一个 **source key**，真正的候选由游戏在
///   运行时推快照进来（见 [`CommandRegistry::set_dynamic`](crate::CommandRegistry::set_dynamic)），
///   比如"当前 player id 列表"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamSource {
    /// 固定的候选列表。
    Static(Vec<Candidate>),
    /// 枚举候选：`name` 是枚举类型名，`variants` 是成员。
    Enum {
        /// 枚举类型名（显示用，比如 `GameMode`）。
        name: String,
        /// 枚举成员。
        variants: Vec<Candidate>,
    },
    /// 动态候选：按 `source` 到注册表的动态快照里取。
    Dynamic {
        /// 候选来源的 key（比如 `"players"`）。
        source: String,
    },
    /// 自由输入：没有候选，只有 [`ValueHint`] 提示。
    Free {
        /// 值类型提示。
        hint: ValueHint,
    },
}

impl ParamSource {
    /// 这个来源的候选是注册时固定的吗（静态 / 枚举都是）。
    pub fn is_static(&self) -> bool {
        matches!(self, ParamSource::Static(_) | ParamSource::Enum { .. })
    }

    /// 这个来源自己的候选（动态来源返回空，得去注册表取）。
    pub fn own_candidates(&self) -> &[Candidate] {
        match self {
            ParamSource::Static(candidates) => candidates,
            ParamSource::Enum { variants, .. } => variants,
            ParamSource::Dynamic { .. } | ParamSource::Free { .. } => &[],
        }
    }
}

/// 一个参数的形状。
///
/// 位置参数（不按名字输入）：`name` 是给界面看的提示，也是 [`CommandRegistry::resolve`]
/// 里结果的 key。
///
/// [`CommandRegistry::resolve`]: crate::CommandRegistry::resolve
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    /// 参数名（提示 + 解析结果的 key）。
    pub name: String,
    /// 说明文字。
    pub tooltip: Option<String>,
    /// 候选值来源。
    pub source: ParamSource,
    /// 值类型提示：`Free` 时用来解析文本，有候选时只影响显示。
    pub hint: ValueHint,
    /// 可选参数（`[count]`）：缺省也合法。
    pub optional: bool,
    /// 变长参数（`<item>...`）：可以重复出现。
    pub variadic: bool,
}

impl ParamSpec {
    /// 一个自由输入参数（数字 / 文本 / 布尔）。
    pub fn free(name: impl Into<String>, hint: ValueHint) -> Self {
        Self {
            name: name.into(),
            tooltip: None,
            source: ParamSource::Free { hint },
            hint,
            optional: false,
            variadic: false,
        }
    }

    /// 一个固定候选列表的参数。
    pub fn choices(
        name: impl Into<String>,
        candidates: impl IntoIterator<Item = impl Into<Candidate>>,
    ) -> Self {
        Self {
            name: name.into(),
            tooltip: None,
            source: ParamSource::Static(candidates.into_iter().map(Into::into).collect()),
            hint: ValueHint::Text,
            optional: false,
            variadic: false,
        }
    }

    /// 一个枚举参数：`variants` 是枚举成员，`enum_name` 是类型名（显示用）。
    pub fn variants(
        name: impl Into<String>,
        enum_name: impl Into<String>,
        variants: impl IntoIterator<Item = impl Into<Candidate>>,
    ) -> Self {
        Self {
            name: name.into(),
            tooltip: None,
            source: ParamSource::Enum {
                name: enum_name.into(),
                variants: variants.into_iter().map(Into::into).collect(),
            },
            hint: ValueHint::Text,
            optional: false,
            variadic: false,
        }
    }

    /// 一个动态候选参数：候选由游戏运行时按 `source` 推快照进来。
    pub fn dynamic(name: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tooltip: None,
            source: ParamSource::Dynamic {
                source: source.into(),
            },
            hint: ValueHint::Text,
            optional: false,
            variadic: false,
        }
    }

    /// 附上说明文字。
    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// 改成可选参数。
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// 改成变长参数。
    pub fn variadic(mut self) -> Self {
        self.variadic = true;
        self
    }

    /// 覆盖值类型提示。
    pub fn with_hint(mut self, hint: ValueHint) -> Self {
        self.hint = hint;
        self
    }

    /// 参数在用法串里的写法：`<name>` / `[name]`，变长加 `...`。
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

    /// 解析一个值。
    ///
    /// `dynamic` 是**这个参数对应的**动态候选快照（静态 / 枚举参数会忽略它）：
    /// 动态候选为空时按自由文本接受（快照可能只是还没刷新），非空时要求命中。
    pub fn parse(&self, text: &str, dynamic: &[Candidate]) -> Result<Value, ValueParseError> {
        // 变长参数只会解析单个值，重复由 `resolve` 负责。
        match &self.source {
            ParamSource::Static(candidates) => {
                match_candidate(candidates, text).ok_or_else(|| ValueParseError::NotAChoice {
                    value: text.to_string(),
                    expected: candidates.iter().map(|c| c.value.clone()).collect(),
                })
            }
            ParamSource::Enum { variants, .. } => {
                match_candidate(variants, text).ok_or_else(|| ValueParseError::NotAChoice {
                    value: text.to_string(),
                    expected: variants.iter().map(|c| c.value.clone()).collect(),
                })
            }
            ParamSource::Dynamic { .. } => {
                if dynamic.is_empty() {
                    Ok(Value::Text(text.to_string()))
                } else {
                    match_candidate(dynamic, text).ok_or_else(|| ValueParseError::NotAChoice {
                        value: text.to_string(),
                        expected: dynamic.iter().map(|c| c.value.clone()).collect(),
                    })
                }
            }
            ParamSource::Free { hint } => parse_free(text, *hint),
        }
    }
}

/// 按名字（大小写不敏感）在候选里找，返回**规范写法**。
fn match_candidate(candidates: &[Candidate], text: &str) -> Option<Value> {
    candidates
        .iter()
        .find(|candidate| candidate.value.eq_ignore_ascii_case(text))
        .map(|candidate| Value::Text(candidate.value.clone()))
}

/// 按类型提示解析一段自由文本。
pub fn parse_free(text: &str, hint: ValueHint) -> Result<Value, ValueParseError> {
    match hint {
        ValueHint::Any | ValueHint::Text | ValueHint::QuotedText => {
            Ok(Value::Text(text.to_string()))
        }
        ValueHint::Integer => text
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| ValueParseError::NotAnInteger(text.to_string())),
        ValueHint::Float => text
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| ValueParseError::NotAFloat(text.to_string())),
        ValueHint::Bool => match text.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Ok(Value::Bool(true)),
            "false" | "no" | "off" | "0" => Ok(Value::Bool(false)),
            _ => Err(ValueParseError::NotABool(text.to_string())),
        },
    }
}

/// 一个参数值解析失败的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueParseError {
    /// 不在候选列表里。
    NotAChoice {
        /// 输入的值。
        value: String,
        /// 期望的候选。
        expected: Vec<String>,
    },
    /// 不是整数。
    NotAnInteger(String),
    /// 不是小数。
    NotAFloat(String),
    /// 不是布尔。
    NotABool(String),
}

impl std::fmt::Display for ValueParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueParseError::NotAChoice { value, expected } => {
                write!(f, "无效的值 `{value}`（可选：{}）", expected.join(" / "))
            }
            ValueParseError::NotAnInteger(value) => write!(f, "`{value}` 不是整数"),
            ValueParseError::NotAFloat(value) => write!(f, "`{value}` 不是小数"),
            ValueParseError::NotABool(value) => write!(f, "`{value}` 不是布尔值（true / false）"),
        }
    }
}

impl std::error::Error for ValueParseError {}

/// 参数树上的一个节点。
///
/// 命令的参数**不是**一条平铺的列表，而是一棵小树 —— 这是 MC 那套命令的关键：
/// 一个字面量分支可以带出*不同*的后续参数。
///
/// ```text
/// time ─┬─ set ─── <value>      // /time set <day|night|数字>
///       └─ query                // /time query
/// ```
///
/// 光有平铺列表做不到这件事（`/time query day` 是错的）。树只在需要分支时才用
/// 得上，线性的命令看一眼 [`CommandSpec::param`] 就够了。
///
/// "能在这里结束"用**没有子节点**表示：`query` 没有孩子，所以 `/time query` 是
/// 一条完整命令；`set` 有孩子，所以 `/time set` 还缺参数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandNode {
    /// 必须原样输入的字面量分支（`set` / `add` / `query`）。
    Literal {
        /// 字面量文本。
        name: String,
        /// 说明文字。
        tooltip: Option<String>,
        /// 这个分支之后还能接什么。
        children: Vec<CommandNode>,
    },
    /// 消费一个 token 的参数，然后进入 `children`。
    Argument {
        /// 参数形状。
        spec: ParamSpec,
        /// 这个参数之后还能接什么。
        children: Vec<CommandNode>,
    },
}

impl CommandNode {
    /// 一个字面量分支。
    pub fn literal(name: impl Into<String>) -> Self {
        CommandNode::Literal {
            name: name.into(),
            tooltip: None,
            children: Vec::new(),
        }
    }

    /// 一个参数节点。
    pub fn argument(spec: ParamSpec) -> Self {
        CommandNode::Argument {
            spec,
            children: Vec::new(),
        }
    }

    /// 附上说明文字。
    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        match &mut self {
            CommandNode::Literal { tooltip: slot, .. } => *slot = Some(tooltip.into()),
            CommandNode::Argument { spec, .. } => spec.tooltip = Some(tooltip.into()),
        }
        self
    }

    /// 在**这条链的末尾**接一个子节点，返回自己。
    ///
    /// `a.then(b).then(c)` 得到 `a → b → c`（不是 `a → {b, c}`）：它先走到最深的
    /// 那个节点再挂上去，所以链式写法读起来就是参数顺序。
    /// 想开分支用 [`CommandSpec::branch`]。
    pub fn then(self, child: CommandNode) -> Self {
        let mut node = self;
        node.push_tail(child);
        node
    }

    /// [`CommandNode::then`] 的参数版写法。
    pub fn then_param(self, spec: ParamSpec) -> Self {
        self.then(CommandNode::argument(spec))
    }

    /// [`CommandNode::then`] 的字面量版写法。
    pub fn then_literal(self, name: impl Into<String>) -> Self {
        self.then(CommandNode::literal(name))
    }

    /// 把 `child` 挂到这条链的最深处。
    fn push_tail(&mut self, child: CommandNode) {
        let children = self.children_mut();
        // 不用 `last_mut()` + `push()` 的 match：两个分支同时借 `children` 会被
        // 借用检查拒绝，先取长度再按下标访问就没有这个问题。
        match children.len() {
            0 => children.push(child),
            len => children[len - 1].push_tail(child),
        }
    }

    /// 子节点（"这个位置之后还能接什么"）。
    pub fn children(&self) -> &[CommandNode] {
        match self {
            CommandNode::Literal { children, .. } | CommandNode::Argument { children, .. } => {
                children
            }
        }
    }

    /// 子节点的可变引用。
    pub fn children_mut(&mut self) -> &mut Vec<CommandNode> {
        match self {
            CommandNode::Literal { children, .. } | CommandNode::Argument { children, .. } => {
                children
            }
        }
    }

    /// 这个节点在输入串里"叫什么"：字面量就是它自己，参数就是参数名。
    pub fn name(&self) -> &str {
        match self {
            CommandNode::Literal { name, .. } => name,
            CommandNode::Argument { spec, .. } => &spec.name,
        }
    }

    /// 说明文字。
    pub fn tooltip(&self) -> Option<&str> {
        match self {
            CommandNode::Literal { tooltip, .. } => tooltip.as_deref(),
            CommandNode::Argument { spec, .. } => spec.tooltip.as_deref(),
        }
    }

    /// 这是个参数节点吗？是的话返回它的形状。
    pub fn argument_spec(&self) -> Option<&ParamSpec> {
        match self {
            CommandNode::Argument { spec, .. } => Some(spec),
            CommandNode::Literal { .. } => None,
        }
    }

    /// 这是个字面量分支吗？
    pub fn is_literal(&self) -> bool {
        matches!(self, CommandNode::Literal { .. })
    }

    /// 这里能结束吗（叶子 = 一条完整命令）。
    pub fn is_terminal(&self) -> bool {
        self.children().is_empty()
    }
}

/// 一个命令的形状：名字、别名、说明、参数树，以及（可选的）执行体。
///
/// 没有 `PartialEq`：`run` 是函数指针，比较它没有意义（编译器也会为此报警告）。
#[derive(Clone, Debug)]
pub struct CommandSpec {
    /// 命令名（主名）。
    pub name: String,
    /// 别名。
    pub aliases: Vec<String>,
    /// 说明文字。
    pub description: Option<String>,
    /// 参数树的根：第一个位置可以有哪些节点。
    pub nodes: Vec<CommandNode>,
    /// **纯命令**的执行体（见 [`CommandFn`]）：拿不到外部状态，只有解析好的调用。
    ///
    /// `None` 表示这条命令由调用方自己接执行体（比如需要 `&mut GameState` 的命令），
    /// `#[command]` 宏在函数没有上下文参数时会自动把它填上。
    pub run: Option<CommandFn>,
}

impl CommandSpec {
    /// 一个没有参数、也没有执行体的命令。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            aliases: Vec::new(),
            description: None,
            nodes: Vec::new(),
            run: None,
        }
    }

    /// 接上纯命令的执行体（`#[command]` 生成 `spec()` 时用）。
    pub fn with_run(mut self, run: CommandFn) -> Self {
        self.run = Some(run);
        self
    }

    /// 加一个别名。
    pub fn alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// 附上说明文字。
    pub fn describe(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 在参数链的最后追加一个参数。
    pub fn param(mut self, spec: ParamSpec) -> Self {
        self.push_tail(CommandNode::argument(spec));
        self
    }

    /// 在参数链的最后追加一个字面量。
    pub fn literal(mut self, name: impl Into<String>) -> Self {
        self.push_tail(CommandNode::literal(name));
        self
    }

    /// 在**根部**追加一个分支（`/time <set ...> <query>` 的 set / query）。
    pub fn branch(mut self, node: CommandNode) -> Self {
        self.nodes.push(node);
        self
    }

    /// 在参数链的最后追加一个节点（需要同时带孩子的场合）。
    pub fn then(mut self, node: CommandNode) -> Self {
        self.push_tail(node);
        self
    }

    /// 把 `node` 挂到这条链的最深处。
    fn push_tail(&mut self, node: CommandNode) {
        // 和 `CommandNode::push_tail` 一样：先取长度，避免两个分支同时可变借。
        match self.nodes.len() {
            0 => self.nodes.push(node),
            len => self.nodes[len - 1].push_tail(node),
        }
    }

    /// `name` 是这个命令的主名或某个别名吗？
    pub fn matches_name(&self, name: &str, case_sensitive: bool) -> bool {
        let eq = |a: &str, b: &str| {
            if case_sensitive {
                a == b
            } else {
                a.eq_ignore_ascii_case(b)
            }
        };
        eq(&self.name, name) || self.aliases.iter().any(|alias| eq(alias, name))
    }

    /// 用法串：命令名 + 沿第一条分支走到底的槽位，比如
    /// `give <player_id> <item> [count]`。
    pub fn usage(&self) -> String {
        let mut usage = self.name.clone();
        let mut nodes = &self.nodes[..];
        while let Some(first) = nodes.first() {
            usage.push(' ');
            match first {
                CommandNode::Literal { name, .. } => usage.push_str(name),
                CommandNode::Argument { spec, .. } => usage.push_str(&spec.slot()),
            }
            nodes = first.children();
        }
        usage
    }

    /// 第一条分支上的全部槽位名（参数名 / 字面量），按顺序。
    pub fn slot_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut nodes = &self.nodes[..];
        while let Some(first) = nodes.first() {
            names.push(first.name().to_string());
            nodes = first.children();
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(spec: &CommandSpec) -> Vec<&str> {
        spec.nodes
            .first()
            .map(|node| {
                let mut names = vec![node.name()];
                let mut node = node;
                while let Some(next) = node.children().first() {
                    names.push(next.name());
                    node = next;
                }
                names
            })
            .unwrap_or_default()
    }

    #[test]
    fn params_build_a_linear_chain() {
        let spec = CommandSpec::new("give")
            .param(ParamSpec::dynamic("player_id", "players"))
            .param(ParamSpec::choices("item", ["sword"]))
            .param(ParamSpec::free("count", ValueHint::Integer).optional());
        assert_eq!(chain(&spec), ["player_id", "item", "count"]);
        assert_eq!(spec.usage(), "give <player_id> <item> [count]");
        assert_eq!(spec.slot_names(), ["player_id", "item", "count"]);
    }

    #[test]
    fn branches_sit_side_by_side_at_the_root() {
        let spec = CommandSpec::new("time")
            .branch(
                CommandNode::literal("set")
                    .then_param(ParamSpec::free("value", ValueHint::Integer)),
            )
            .branch(CommandNode::literal("query"));
        assert_eq!(spec.nodes.len(), 2);
        assert_eq!(spec.nodes[0].name(), "set");
        assert!(!spec.nodes[0].is_terminal());
        assert!(spec.nodes[1].is_terminal());
    }

    #[test]
    fn variants_are_matched_and_normalized() {
        let spec = ParamSpec::variants("mode", "GameMode", ["creative", "survival"]);
        assert_eq!(
            spec.parse("CREATIVE", &[]),
            Ok(Value::Text("creative".into()))
        );
        assert!(spec.parse("nope", &[]).is_err());
    }

    #[test]
    fn dynamic_params_accept_anything_until_a_snapshot_arrives() {
        let spec = ParamSpec::dynamic("player_id", "players");
        assert_eq!(spec.parse("p1", &[]), Ok(Value::Text("p1".into())));
        let snapshot = [Candidate::new("p1"), Candidate::new("p2")];
        assert!(spec.parse("p9", &snapshot).is_err());
    }

    #[test]
    fn free_params_parse_by_hint() {
        assert_eq!(
            ParamSpec::free("count", ValueHint::Integer).parse("-3", &[]),
            Ok(Value::Integer(-3))
        );
        assert_eq!(
            ParamSpec::free("on", ValueHint::Bool).parse("TRUE", &[]),
            Ok(Value::Bool(true))
        );
        assert!(ParamSpec::free("count", ValueHint::Integer)
            .parse("x", &[])
            .is_err());
    }
}
