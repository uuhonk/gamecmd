//! `gamecmd` — 游戏命令模式（`/` 命令）的**纯逻辑**核心。
//!
//! 这个 crate 只做三件事，别的什么都不做：
//!
//! 1. **注册**命令和它们的参数（[`CommandRegistry`]、[`CommandSpec`]、
//!    [`ParamSpec`]）：参数候选可以是静态列表、枚举成员，也可以是一个
//!    **动态快照**（"当前的 player id 列表"这种每个游戏不一样、还会变的数据）；
//! 2. **解析**输入串（[`parse`]）：按空白切 token，支持双引号包住的带空格值，
//!    保留光标位置；
//! 3. **补全**（[`CommandRegistry::complete`]）：根据当前输入返回命令候选 / 参数名
//!    提示 / 参数值候选，并用一套手写的模糊匹配打分排序。
//!
//! 它**不认识 UI，也不认识 ECS**：不依赖任何引擎（只依赖 glam 向量和自带的
//! proc-macro），不创建任何对象，不读文件，不打印日志。界面层负责把输入串喂进来、
//! 把 [`Completion`] 画出来，数据层负责在状态变化时调用
//! [`CommandRegistry::set_dynamic`] 刷新动态候选。
//!
//! # 心智模型（MC 风格）
//!
//! ```text
//! "/give @p minecraft:diamond 3"
//!   │      │  │                └── 参数值（自由输入）
//!   │      │  └─────────────────── 参数值（动态候选：当前物品表）
//!   │      └────────────────────── 参数值（动态候选：当前玩家列表）
//!   └───────────────────────────── 命令名（还没打第一个空格时就是"命令候选模式"）
//! ```
//!
//! - 输入里**还没有出现第一个空格** → [`Phase::Command`]：候选是整个命令表，
//!   按命令名（和别名）模糊匹配；
//! - 出现第一个空格、而且命令名在注册表里 → [`Phase::Argument`]：返回
//!   [`Completion::params`]（该命令的参数名 / 字面量分支列表，带"当前是哪一个"的
//!   标记）以及 [`Completion::candidates`]（当前这个参数的可选值）；
//! - 命令不认识、或者参数对不上 → [`Phase::Invalid`]：候选通常为空，
//!   [`Completion::error`] 里是可以直接显示的说明。
//!
//! 参数不是"必须按名字输入"的（和 MC 一样是位置参数），`params` 是**提示**，
//! 不是语法的一部分；真正被输入的是每个位置上的值。
//!
//! # 例子
//!
//! ```
//! use gamecmd::{CommandRegistry, CommandSpec, ParamSpec, Phase, ValueHint};
//!
//! let mut registry = CommandRegistry::new();
//! registry.register(
//!     CommandSpec::new("give")
//!         .describe("给一名玩家一件物品")
//!         .param(ParamSpec::dynamic("player_id", "players")) // 动态候选
//!         .param(ParamSpec::choices("item", ["sword", "cash"]))
//!         .param(ParamSpec::free("count", ValueHint::Integer).optional()),
//! );
//! // 游戏每帧（或数据变化时）把当前玩家 id 推给注册表。
//! registry.set_dynamic("players", ["p1", "p2"]);
//!
//! // 还没打空格：命令候选模式。
//! let c = registry.complete("gi", 2);
//! assert_eq!(c.phase, Phase::Command);
//! assert_eq!(c.candidates[0].candidate.value, "give");
//!
//! // 命令 + 空格：参数名提示 + 第一个参数的可选值。
//! let c = registry.complete("give ", 5);
//! assert_eq!(c.phase, Phase::Argument);
//! assert_eq!(c.params.len(), 3);
//! assert_eq!(c.param_index, Some(0));
//! assert_eq!(c.candidates.len(), 2); // p1 / p2
//!
//! // 走完参数树，拿到结构化结果。
//! let invocation = registry.resolve("give p1 sword 3").unwrap();
//! assert_eq!(invocation.command, "give");
//! assert_eq!(invocation.args.len(), 3);
//! ```
//!
//! # 第二阶段：`#[command]` + `CommandParam`（一个函数就是一条命令）
//!
//! 标一个函数，宏按签名生成"注册 + 拆包 + 执行"三件事；参数类型实现
//! [`CommandParam`] 就自动**平铺**成命令列 —— `Vec3` 铺成三列，`Option<T>` 变成可选：
//!
//! ```ignore
//! #[command("tp", "把角色传送到一个坐标", alias = "teleport")]
//! fn tp(state: &mut GameState, target: PlayerRef, position: Position) -> Result<String, String> {
//!     // target / position 已经拆好了
//! }
//!
//! // 生成 tp::spec() / tp::args() / tp::run(invocation, state)
//! registry.register(tp::spec());
//! ```
//!
//! ```text
//! tp @s 100 200 300        PlayerRef 占 1 列，Position 占 3 列
//! ```
//!
//! 基础类型（整数 / 浮点 / `bool` / `String` / `Option<T>`）和 **glam 向量**
//! （`Vec2` / `Vec3` / `Vec4` / `IVec2` / `IVec3`）的实现在这个 crate 里，所以签名里
//! 直接写 `position: Vec3` 就行；自己定义的类型在自己的 crate 里实现
//! （见 [`CommandParam`]，注意 Rust 的孤儿规则：外部 trait 不能给外部类型实现）。
//!
//! 有两个注册入口：
//!
//! - **纯命令**（没有上下文参数）：`entry()` 自带执行体，
//!   `register_commands!(registry; help, echo)` 一行注册一批；
//! - **带上下文的命令**（第一个参数是 `&mut T`）：调用方自己接执行体 —— 用
//!   `tp::spec()` 注册、用 `tp::run(invocation, state)` 执行。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

// `#[command]` 生成的代码用 `::gamecmd::…` 引用运行时，所以 crate 内部也要能用这个名字
// 找到自己（这样 crate 内也能直接给宏写测试）。
extern crate self as gamecmd;

pub mod candidate;
pub mod matcher;
pub mod param;
pub mod parse;
pub mod registry;
pub mod spec;
pub mod suggest;

pub use candidate::{Candidate, Value, ValueHint};
pub use matcher::Match;
pub use param::{CommandParam, CommandReturn, ParamError};
pub use parse::{parse, ParsedInput, Token};
pub use registry::{CommandError, CommandFn, CommandRegistry, Invocation, InvocationArg};
pub use spec::{CommandNode, CommandSpec, ParamSource, ParamSpec};
pub use suggest::{
    CompleteOptions, Completion, ParamHint, Phase, SourceKind, Suggestion, SuggestionKind,
};

/// 按函数签名生成命令的注册 / 拆包 / 执行代码 —— 见 [`CommandParam`]。
pub use gamecmd_derive::command;

/// 注册一批**纯命令**（`#[command]` 标记过、且没有上下文参数）。
///
/// ```ignore
/// gamecmd::register_commands!(registry; help, echo);
/// // 展开成：registry.register(help::entry()); registry.register(echo::entry());
/// ```
///
/// 带上下文的命令不能用它（那些 `run` 需要宿主传上下文，没有自带的 `entry()`）。
#[macro_export]
macro_rules! register_commands {
    ($registry:expr; $($command:ident),* $(,)?) => {{
        $(
            $registry.register($command::entry());
        )*
    }};
}

#[cfg(test)]
mod macro_tests {
    //! `#[command]` 的端到端测试：标记函数 → 注册 → 补全 → 拆包 → 执行。

    use super::*;

    /// 一个"复杂类型"，铺成三列（和内置的 `Vec3` 是同一套做法）。
    #[derive(Debug, PartialEq)]
    struct Triple(f32, f32, f32);

    impl CommandParam for Triple {
        const ARITY: usize = 3;

        fn param_specs(name: &str) -> Vec<ParamSpec> {
            vec![
                ParamSpec::free(format!("{name}.x"), ValueHint::Float),
                ParamSpec::free(format!("{name}.y"), ValueHint::Float),
                ParamSpec::free(format!("{name}.z"), ValueHint::Float),
            ]
        }

        fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
            if columns.len() < 3 {
                return Err(ParamError::Missing {
                    needed: 3,
                    got: columns.len(),
                });
            }
            let value = |index: usize| columns[index].value.as_f64().unwrap_or_default() as f32;
            Ok((Triple(value(0), value(1), value(2)), 3))
        }
    }

    #[command("tp", "把角色传送到一个坐标", alias = "teleport")]
    fn tp(target: String, position: Triple) -> String {
        format!("{target} -> {}, {}, {}", position.0, position.1, position.2)
    }

    #[command("boom")]
    fn boom() -> Result<(), String> {
        Err("炸了".into())
    }

    #[command]
    fn ping() -> &'static str {
        "pong"
    }

    #[command("bump")]
    fn bump(context: &mut u32, count: u32) -> String {
        *context += count;
        format!("now {context}")
    }

    #[test]
    fn a_marked_function_becomes_a_registered_command() {
        let mut registry = CommandRegistry::new();
        register_commands!(registry; tp, boom, ping);

        let spec = registry.get("tp").unwrap();
        assert_eq!(spec.aliases, ["teleport"]);
        assert_eq!(
            spec.usage(),
            "tp <target> <position.x> <position.y> <position.z>"
        );

        assert_eq!(
            registry.execute("tp @s 1 2 3").unwrap(),
            Some("@s -> 1, 2, 3".into())
        );
        // 别名走同一条路。
        assert_eq!(
            registry.execute("teleport @s 4 5 6").unwrap(),
            Some("@s -> 4, 5, 6".into())
        );
        // 参数不够：解析先拦下来。
        assert!(matches!(
            registry.execute("tp @s 1 2"),
            Err(CommandError::MissingArgument { .. })
        ));
        // 命令体的 `Err` 变成命令错误。
        assert_eq!(
            registry.execute("boom"),
            Err(CommandError::Execution("炸了".into()))
        );
        // 没有参数、按函数名注册、返回 `&str`。
        assert_eq!(registry.execute("ping").unwrap(), Some("pong".into()));
    }

    #[test]
    fn a_context_parameter_is_passed_through_and_does_not_take_a_column() {
        let spec = bump::spec();
        // 带上下文的命令由宿主接执行体，宏不会自动挂。
        assert!(spec.run.is_none());
        assert_eq!(spec.usage(), "bump <count>");

        let mut registry = CommandRegistry::new();
        registry.register(spec);
        let invocation = registry.resolve("bump 5").unwrap();
        let mut context = 1u32;
        assert_eq!(
            bump::run(&invocation, &mut context).unwrap(),
            Some("now 6".into())
        );
        assert_eq!(context, 6);
    }

    #[test]
    fn a_marked_command_completes_its_flattened_columns() {
        let mut registry = CommandRegistry::new();
        register_commands!(registry; tp);
        let completion = registry.complete("tp @s ", 6);
        assert_eq!(
            completion
                .params
                .iter()
                .map(|hint| hint.name.as_str())
                .collect::<Vec<_>>(),
            ["target", "position.x", "position.y", "position.z"]
        );
    }
}
