# gamecmd

游戏命令模式（`/` 命令）的**纯逻辑**核心 —— 命令注册表 + 参数候选 + 模糊补全 +
`#[command]` 函数注册宏。核心不依赖任何引擎、也不依赖别的 crate（依赖只有自己的
proc-macro crate `gamecmd_derive` 和 **glam** —— 游戏里的坐标几乎都是 glam 向量，
把最常见的一类参数拦在门外没有意义），所以它可以单独 `cargo test`。

它只做四件事：

| 做 | 不做 |
| --- | --- |
| 注册命令和参数的形状 | 不认识 UI / 引擎 |
| 切分输入串（引号、光标） | 不创建任何对象 |
| 按当前输入算候选 / 参数提示 | 不碰文件、不打印日志 |
| 按 `CommandParam` 把参数平铺 / 收回 | 命令正文由使用方写（宏只生成注册与拆包） |

## 一个函数就是一条命令（第二阶段）

```rust
use gamecmd::{command, CommandRegistry};

#[command("tp", "把角色传送到一个坐标", alias = "teleport")]
fn tp(target: String, position: Vec3) -> String {
    format!("{target} @ {position}")
}

let mut registry = CommandRegistry::new();
registry.register(tp::spec());          // 也支持 gamecmd::register_commands!(registry; tp)
assert_eq!(
    tp::spec().usage(),
    "tp <target> <position.x> <position.y> <position.z>"
);
```

```text
tp @s 100 200 300      ← Vec3 吃掉三列，收回一个值
```

宏为 `fn tp` 生成一个**同名模块**（模块在类型命名空间、函数在值命名空间，可以同名）：

| 生成项 | 干什么 |
| --- | --- |
| `tp::spec()` | 注册用：按每个形参的 `CommandParam` 平铺出参数列 |
| `tp::args(invocation)` | 按形参类型拆包，返回元组 |
| `tp::run(invocation)` | 拆包 → 调原函数（第一个参数是 `&mut T` 等上下文时，它由调用方传进来） |
| `tp::entry()` | 只有**纯命令**才生成：`spec` + 执行体都齐了 |

`CommandParam` 已经覆盖：整数 / 浮点 / `bool` / `String` / `Option<T>`，以及
**glam 向量** `Vec2` / `Vec3` / `Vec4`（每列小数）和 `IVec2` / `IVec3`（每列整数）——
上面那个 `Vec3` 就是内置的。

自己定义的类型在自己的 crate 里实现 `CommandParam`，比如带动态候选的"玩家 id"：

```ignore
struct PlayerRef(String);

impl CommandParam for PlayerRef {
    const ARITY: usize = 1;
    fn param_specs(name: &str) -> Vec<ParamSpec> {
        vec![ParamSpec::dynamic(name, "players")]   // 候选由游戏运行时推上来
    }
    fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
        // …
    }
}
```

注意 Rust 的孤儿规则：**外部**类型（别的库里的 `Uuid`、`Duration`……）没法在使用方
实现这个 trait，要么包一层本地 newtype，要么把实现放进那个类型所在的 crate。

## 快速上手

```rust
use gamecmd::{Candidate, CommandNode, CommandRegistry, CommandSpec, ParamSpec, ValueHint};

let mut registry = CommandRegistry::new();
registry.register(
    CommandSpec::new("give")
        .alias("g")
        .describe("给一名玩家一件物品")
        .param(ParamSpec::dynamic("player_id", "players"))  // 动态候选
        .param(ParamSpec::choices("item", ["sword", "cash"])) // 静态候选
        .param(ParamSpec::free("count", ValueHint::Integer).optional()),
);
registry.register(
    CommandSpec::new("time")
        .branch(CommandNode::literal("set").then_param(ParamSpec::choices("value", ["day", "night"])))
        .branch(CommandNode::literal("query")),
);

// 动态候选：游戏在数据变化时推一份快照进来（"当前 player id 列表"）。
registry.set_dynamic("players", ["p1", "p2"]);

// 补全：还没打空格 → 命令候选；打了空格 → 参数提示 + 第一个参数的可选值。
let completion = registry.complete("give ", 5);
assert_eq!(completion.usage.as_deref(), Some("give <player_id> <item> [count]"));
assert_eq!(completion.param_index, Some(0));
assert_eq!(completion.candidates.len(), 2);          // p1 / p2
let (input, cursor) = completion.apply(0).unwrap();   // Tab 接受第一条
assert_eq!(input, "give p1 ");

// 解析：把一条完整的输入变成结构化结果。
let invocation = registry.resolve("give p1 sword 3").unwrap();
assert_eq!(invocation.value("count"), Some(&gamecmd::Value::Integer(3)));
```

## 命令的形状

```text
"/give @p minecraft:diamond 3"
  │      │  │                └── 参数值（自由输入）
  │      │  └─────────────────── 参数值（动态候选：当前物品表）
  │      └────────────────────── 参数值（动态候选：当前玩家列表）
  └───────────────────────────── 命令名（还没打第一个空格 = 命令候选模式）
```

参数是**位置**参数（和 MC 一样），参数名只是提示。需要"不同的字面量分支带出不同的
后续参数"时用 [`CommandNode`] 建一棵小树：

```text
time ─┬─ set ─── <value>
      └─ query
```

## 三种参数候选来源

| 来源 | 构造 | 候选从哪来 |
| --- | --- | --- |
| 静态 | `ParamSpec::choices` | 注册时写死的列表 |
| 枚举 | `ParamSpec::variants("mode", "GameMode", [...])` | 枚举成员（额外记住类型名给界面显示） |
| 动态 | `ParamSpec::dynamic("player_id", "players")` + `set_dynamic` | 游戏运行时推的快照，可随时覆盖 |
| 自由 | `ParamSpec::free("count", ValueHint::Integer)` | 没有候选，只有类型提示 |

## 补全结果

[`Completion`] 一次给出三类信息：

- `phase` —— `Command`（在补命令名）/ `Argument`（在补参数）/ `Invalid`（命令不认识
  或参数对不上）；
- `params` —— 参数名 / 字面量分支的提示，当前槽位带 `current = true`（界面自己拼
  "用法串 + 当前参数"）；
- `candidates` —— 光标当前位置能填的值，已经模糊匹配过滤并排序，`matches` 里带命中
  字符下标供高亮。

排序：**前缀匹配 > 分数高 > 注册顺序**。匹配算法是手写的子序列打分（DP + 位置奖励），
不依赖任何库，规则见 `src/matcher.rs` 的模块文档。

## 执行

命令正文分两种：

- **纯命令**（没有上下文参数）：`spec.run` 由宏填好，
  `gamecmd::register_commands!(registry; help, echo)` 一行注册一批，
  `registry.execute("echo hi")` 直接跑；
- **带上下文的命令**（第一个参数是 `&mut T`，比如 `&mut GameState`）：宏生成
  `run(invocation, context)`，由调用方保管并调用 —— `gamecmd` 不认识那个 `T`，
  所以它只负责注册和拆包。

返回值可以是 `()`、`String` / `&str`（当作要显示的一句话）或 `Result<_, E: Display>`
（`Err` 变成命令错误）。

## 目录

| 文件 | 内容 |
| --- | --- |
| `candidate.rs` | `Candidate` / `ValueHint` / `Value` |
| `spec.rs` | `ParamSpec` / `ParamSource` / `CommandNode` / `CommandSpec` |
| `param.rs` | `CommandParam`（平铺 / 拆包）+ 内置实现 + `CommandReturn` |
| `parse.rs` | 输入串切分（引号、光标） |
| `matcher.rs` | 模糊匹配打分（手写 DP） |
| `suggest.rs` | `Completion` 和补全算法 |
| `registry.rs` | `CommandRegistry` / `Invocation` / `CommandError` / `execute` |
| `derive/` | `#[command]` 属性宏（proc-macro crate）|
