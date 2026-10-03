//! `CommandParam`：一个类型怎么变成命令里的参数列，以及怎么从参数列变回来。
//!
//! 这是"平铺"发生的地方。`#[command]` 宏把函数的每个形参交给它的类型：
//!
//! ```text
//! fn tp(player: PlayerRef, position: Vec3)
//!                  │              │
//!                  │              └── ARITY = 3：铺成 position.x / position.y / position.z 三列
//!                  └───────────────── ARITY = 1：铺成 player 一列（候选来自动态 source）
//! ```
//!
//! 于是函数签名和命令输入对得上：
//!
//! ```text
//! tp @s 100 200 300
//!    │  └──┬──┘
//!    │     └── Vec3::take_columns 把三个值收回一个 Vec3
//!    └──────── PlayerRef::take_columns 拿一个值
//! ```
//!
//! # 内置了什么，怎么自己加
//!
//! 这个 crate 已经实现了：
//!
//! - **标量**：整数（`u8`…`usize` / `i8`…`isize`）、`f32` / `f64`、`bool`、`String`；
//! - **`Option<T>`**：整组列变成可缺省；
//! - **glam 向量**：`Vec2` / `Vec3` / `Vec4`（每列小数）、`IVec2` / `IVec3`（每列整数），
//!   按分量铺开 —— 所以签名里可以直接写 `position: Vec3`。
//!
//! 自己的类型由自己实现（比如"玩家 id"这种带动态候选的类型）：
//!
//! ```ignore
//! struct PlayerRef(String);
//!
//! impl CommandParam for PlayerRef {
//!     const ARITY: usize = 1;
//!     fn param_specs(name: &str) -> Vec<ParamSpec> {
//!         // 候选来自运行时推上来的动态 source。
//!         vec![ParamSpec::dynamic(name, "players")]
//!     }
//!     fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
//!         let Some(column) = columns.first() else {
//!             return Err(ParamError::Missing { needed: 1, got: 0 });
//!         };
//!         Ok((PlayerRef(column.value.to_string()), 1))
//!     }
//! }
//! ```
//!
//! 注意 Rust 的**孤儿规则**：外部 trait 不能给外部类型实现。glam 类型之所以能在
//! 这里实现，是因为 `gamecmd` 自己依赖 glam（见 `Cargo.toml`）—— 用别人的类型做
//! 命令参数时，要么包一层本地 newtype，要么把实现放进那个类型所在的 crate。

use crate::candidate::ValueHint;
use crate::registry::{CommandError, Invocation, InvocationArg};
use crate::spec::ParamSpec;

/// 一个类型在命令里的参数列数 / 解析方式。
///
/// 只有两个必须实现的东西：[`ARITY`](CommandParam::ARITY) 和
/// [`param_specs`](CommandParam::param_specs) 决定"铺开成什么"，[`take_columns`]
/// (CommandParam::take_columns) 决定"怎么收回来"。
pub trait CommandParam: Sized {
    /// 这个类型在命令里铺开成几列（`Vec3` = 3，`i32` = 1）。
    ///
    /// 必须等于 [`param_specs`](CommandParam::param_specs) 返回的长度。
    const ARITY: usize;

    /// 铺开后的参数形状。
    ///
    /// `name` 是形参名；复杂类型应该给每一列取一个能区分的名字
    /// （`position.x` / `position.y` / `position.z`）。
    fn param_specs(name: &str) -> Vec<ParamSpec>;

    /// 从**剩余的**参数列里构造自己，返回用掉了几列。
    ///
    /// `columns` 是整个调用里从这个参数开始的切片，可能比
    /// [`ARITY`](CommandParam::ARITY) 短（末尾的可选参数没填）—— 这时返回
    /// [`ParamError::Missing`]。
    fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError>;

    /// 从一次 [`Invocation`] 里取自己（把 `column` 往后推
    /// [`ARITY`](CommandParam::ARITY) 列）。`#[command]` 生成的执行体用它。
    fn from_invocation(invocation: &Invocation, column: &mut usize) -> Result<Self, CommandError> {
        let rest = invocation.args.get(*column..).unwrap_or(&[]);
        let (value, used) = Self::take_columns(rest).map_err(|error| {
            let (param, raw) = match rest.first() {
                Some(arg) => (arg.name.clone(), arg.raw.clone()),
                None => (format!("#{column}"), String::new()),
            };
            CommandError::InvalidValue {
                command: invocation.command.clone(),
                param,
                value: raw,
                reason: error.to_string(),
            }
        })?;
        *column += used;
        Ok(value)
    }
}

/// 一个参数列没法变成目标类型。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamError {
    /// 列不够：还需要 `needed` 列，只剩 `got` 列。
    Missing {
        /// 需要几列。
        needed: usize,
        /// 还剩几列。
        got: usize,
    },
    /// 值本身不合法。
    Invalid(String),
}

impl ParamError {
    /// 一个"值不合法"的错误。
    pub fn invalid(message: impl Into<String>) -> Self {
        ParamError::Invalid(message.into())
    }
}

impl std::fmt::Display for ParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParamError::Missing { needed, got } => {
                write!(f, "缺少参数（需要 {needed} 列，只剩 {got} 列）")
            }
            ParamError::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ParamError {}

/// 命令函数的返回值。
///
/// 支持四种写法（`#[command]` 生成的执行体统一用
/// [`into_outcome`](CommandReturn::into_outcome) 收尾）：
///
/// | 返回类型 | 效果 |
/// | --- | --- |
/// | `()` | 成功，没有消息 |
/// | `String` / `&str` | 成功，并把这句话显示在命令面板的状态行上 |
/// | `Result<(), E>` | `Err` 变成命令错误（`E: Display`） |
/// | `Result<String, E>` | 同上，`Ok` 还带一句消息 |
pub trait CommandReturn {
    /// 变成命令的结局：`Ok(Some(消息))` / `Ok(None)` / `Err(错误)`。
    fn into_outcome(self) -> Result<Option<String>, CommandError>;
}

impl CommandReturn for () {
    fn into_outcome(self) -> Result<Option<String>, CommandError> {
        Ok(None)
    }
}

impl CommandReturn for String {
    fn into_outcome(self) -> Result<Option<String>, CommandError> {
        Ok(Some(self))
    }
}

impl CommandReturn for &str {
    fn into_outcome(self) -> Result<Option<String>, CommandError> {
        Ok(Some(self.to_string()))
    }
}

impl<T, E> CommandReturn for Result<T, E>
where
    T: CommandReturn,
    E: std::fmt::Display,
{
    fn into_outcome(self) -> Result<Option<String>, CommandError> {
        match self {
            Ok(value) => value.into_outcome(),
            Err(error) => Err(CommandError::Execution(error.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// 内置实现
// ---------------------------------------------------------------------------

/// 整数：一列，按 [`ValueHint::Integer`] 解析。
macro_rules! impl_integer_param {
    ($($ty:ty),* $(,)?) => {$(
        impl CommandParam for $ty {
            const ARITY: usize = 1;

            fn param_specs(name: &str) -> Vec<ParamSpec> {
                vec![ParamSpec::free(name, ValueHint::Integer)]
            }

            fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
                let arg = columns.first().ok_or(ParamError::Missing { needed: 1, got: 0 })?;
                let value = arg
                    .value
                    .as_i64()
                    .and_then(|value| <$ty>::try_from(value).ok())
                    .ok_or_else(|| {
                        ParamError::invalid(format!(
                            "`{}` 不是合法的 {}",
                            arg.raw,
                            stringify!($ty)
                        ))
                    })?;
                Ok((value, 1))
            }
        }
    )*};
}

impl_integer_param!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// 浮点：一列，按 [`ValueHint::Float`] 解析。
macro_rules! impl_float_param {
    ($($ty:ty),* $(,)?) => {$(
        impl CommandParam for $ty {
            const ARITY: usize = 1;

            fn param_specs(name: &str) -> Vec<ParamSpec> {
                vec![ParamSpec::free(name, ValueHint::Float)]
            }

            fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
                let arg = columns.first().ok_or(ParamError::Missing { needed: 1, got: 0 })?;
                let value = arg.value.as_f64().ok_or_else(|| {
                    ParamError::invalid(format!("`{}` 不是合法的小数", arg.raw))
                })?;
                Ok((value as $ty, 1))
            }
        }
    )*};
}

impl_float_param!(f32, f64);

impl CommandParam for bool {
    const ARITY: usize = 1;

    fn param_specs(name: &str) -> Vec<ParamSpec> {
        vec![ParamSpec::choices(name, ["true", "false"]).with_hint(ValueHint::Bool)]
    }

    fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
        let arg = columns
            .first()
            .ok_or(ParamError::Missing { needed: 1, got: 0 })?;
        arg.value
            .as_bool()
            .map(|value| (value, 1))
            .ok_or_else(|| ParamError::invalid(format!("`{}` 不是 true / false", arg.raw)))
    }
}

impl CommandParam for String {
    const ARITY: usize = 1;

    fn param_specs(name: &str) -> Vec<ParamSpec> {
        vec![ParamSpec::free(name, ValueHint::Text)]
    }

    fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
        let arg = columns
            .first()
            .ok_or(ParamError::Missing { needed: 1, got: 0 })?;
        // 文本参数原样收；数字 / 布尔也接受（写 `count` 时 `3` 会走整数参数，
        // 但 `String` 参数拿到 `3` 时没有理由报错）。
        Ok((arg.value.to_string(), 1))
    }
}

impl<T: CommandParam> CommandParam for Option<T> {
    const ARITY: usize = T::ARITY;

    fn param_specs(name: &str) -> Vec<ParamSpec> {
        T::param_specs(name)
            .into_iter()
            .map(ParamSpec::optional)
            .collect()
    }

    fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
        // 剩下的列不够铺满 `T` 就当作"没填"。
        if columns.len() < T::ARITY {
            return Ok((None, 0));
        }
        let (value, used) = T::take_columns(columns)?;
        Ok((Some(value), used))
    }
}

// ---------------------------------------------------------------------------
// glam 向量
// ---------------------------------------------------------------------------
//
// 有了它们，命令签名里可以直接写 `position: Vec3`，输入就是
// `tp @s 100 200 300` —— 宏按 `CommandParam` 把它铺成三列。
//
// 依赖 glam 是有意的：游戏里的坐标几乎都是 glam 向量，把最常见的一类参数拦在门外
// 没有意义。如果使用方也依赖 glam，版本必须和这里的一致（见 Cargo.toml），否则会
// 拉进两份 glam、同名的向量类型却对不上。

/// 从 `<N>` 列里读 `N` 个 f32。
fn take_floats<const N: usize>(columns: &[InvocationArg]) -> Result<[f32; N], ParamError> {
    if columns.len() < N {
        return Err(ParamError::Missing {
            needed: N,
            got: columns.len(),
        });
    }
    let mut values = [0.0f32; N];
    for (slot, column) in values.iter_mut().zip(&columns[..N]) {
        *slot = column
            .value
            .as_f64()
            .map(|value| value as f32)
            .ok_or_else(|| ParamError::invalid(format!("`{}` 不是合法的小数", column.raw)))?;
    }
    Ok(values)
}

/// 从 `<N>` 列里读 `N` 个 i32。
fn take_integers<const N: usize>(columns: &[InvocationArg]) -> Result<[i32; N], ParamError> {
    if columns.len() < N {
        return Err(ParamError::Missing {
            needed: N,
            got: columns.len(),
        });
    }
    let mut values = [0i32; N];
    for (slot, column) in values.iter_mut().zip(&columns[..N]) {
        *slot = i32::try_from(column.value.as_i64().unwrap_or(i64::MAX))
            .map_err(|_| ParamError::invalid(format!("`{}` 不是合法的整数", column.raw)))?;
    }
    Ok(values)
}

/// 浮点向量：按分量铺开（`Vec3` → `x` / `y` / `z` 三列）。
macro_rules! impl_float_vector {
    ($ty:ty, $arity:literal, $(($index:literal, $axis:literal)),+ $(,)?) => {
        impl CommandParam for $ty {
            const ARITY: usize = $arity;

            fn param_specs(name: &str) -> Vec<ParamSpec> {
                [
                    $(ParamSpec::free(format!("{name}.{}", $axis), ValueHint::Float)),+
                ]
                .to_vec()
            }

            fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
                let values = take_floats::<$arity>(columns)?;
                Ok((<$ty>::new($(values[$index]),+), $arity))
            }
        }
    };
}

/// 整数向量：和浮点向量一样，只是每列按整数解析。
macro_rules! impl_integer_vector {
    ($ty:ty, $arity:literal, $(($index:literal, $axis:literal)),+ $(,)?) => {
        impl CommandParam for $ty {
            const ARITY: usize = $arity;

            fn param_specs(name: &str) -> Vec<ParamSpec> {
                [
                    $(ParamSpec::free(format!("{name}.{}", $axis), ValueHint::Integer)),+
                ]
                .to_vec()
            }

            fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
                let values = take_integers::<$arity>(columns)?;
                Ok((<$ty>::new($(values[$index]),+), $arity))
            }
        }
    };
}

impl_float_vector!(glam::Vec2, 2, (0, "x"), (1, "y"));
impl_float_vector!(glam::Vec3, 3, (0, "x"), (1, "y"), (2, "z"));
impl_float_vector!(glam::Vec4, 4, (0, "x"), (1, "y"), (2, "z"), (3, "w"));
impl_integer_vector!(glam::IVec2, 2, (0, "x"), (1, "y"));
impl_integer_vector!(glam::IVec3, 3, (0, "x"), (1, "y"), (2, "z"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::CommandRegistry;
    use crate::spec::CommandSpec;

    /// 测试用的"复杂类型"：跟 src 里的 `Vec3` 一样铺成三列。
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
            let mut values = [0.0f32; 3];
            for (slot, column) in values.iter_mut().zip(&columns[..3]) {
                *slot = column.value.as_f64().unwrap_or_default() as f32;
            }
            Ok((Triple(values[0], values[1], values[2]), 3))
        }
    }

    /// 一个自定义的"一列"类型，候选来自动态 source。
    #[derive(Debug, PartialEq)]
    struct Target(String);

    impl CommandParam for Target {
        const ARITY: usize = 1;

        fn param_specs(name: &str) -> Vec<ParamSpec> {
            vec![ParamSpec::dynamic(name, "targets")]
        }

        fn take_columns(columns: &[InvocationArg]) -> Result<(Self, usize), ParamError> {
            let arg = columns
                .first()
                .ok_or(ParamError::Missing { needed: 1, got: 0 })?;
            Ok((Target(arg.value.to_string()), 1))
        }
    }

    fn registry() -> CommandRegistry {
        let mut registry = CommandRegistry::new();
        registry.register(
            CommandSpec::new("tp")
                .param(ParamSpec::dynamic("target", "targets"))
                .param(ParamSpec::free("x", ValueHint::Float))
                .param(ParamSpec::free("y", ValueHint::Float))
                .param(ParamSpec::free("z", ValueHint::Float)),
        );
        registry.set_dynamic("targets", ["@s"]);
        registry.register(
            CommandSpec::new("give")
                .param(ParamSpec::free("item", ValueHint::Text))
                .param(ParamSpec::free("count", ValueHint::Integer).optional()),
        );
        registry
    }

    #[test]
    fn a_scalar_reads_one_column() {
        let invocation = registry().resolve("give sword 3").unwrap();
        let mut column = 0;
        assert_eq!(
            String::from_invocation(&invocation, &mut column).unwrap(),
            "sword"
        );
        assert_eq!(column, 1);
        assert_eq!(u32::from_invocation(&invocation, &mut column).unwrap(), 3);
        assert_eq!(column, 2);
    }

    #[test]
    fn a_complex_type_flattens_over_several_columns() {
        let invocation = registry().resolve("tp @s 100 200 300").unwrap();
        let mut column = 0;
        assert_eq!(
            Target::from_invocation(&invocation, &mut column).unwrap(),
            Target("@s".into())
        );
        assert_eq!(
            Triple::from_invocation(&invocation, &mut column).unwrap(),
            Triple(100.0, 200.0, 300.0)
        );
        assert_eq!(column, 4);
    }

    #[test]
    fn an_optional_type_can_be_missing() {
        let invocation = registry().resolve("give sword").unwrap();
        let mut column = 0;
        assert_eq!(
            String::from_invocation(&invocation, &mut column).unwrap(),
            "sword"
        );
        assert_eq!(
            Option::<u32>::from_invocation(&invocation, &mut column).unwrap(),
            None
        );
        assert_eq!(column, 1);

        let invocation = registry().resolve("give sword 7").unwrap();
        let mut column = 0;
        let _ = String::from_invocation(&invocation, &mut column).unwrap();
        assert_eq!(
            Option::<u32>::from_invocation(&invocation, &mut column).unwrap(),
            Some(7)
        );
    }

    #[test]
    fn a_required_type_reports_the_missing_columns() {
        // 正常解析不会给出"少列"的调用（`resolve` 会先报 MissingArgument），
        // 所以这里手工拼一个残缺的调用，验证拆包侧的报错。
        let invocation = Invocation {
            command: "tp".into(),
            alias: None,
            literals: Vec::new(),
            args: vec![
                InvocationArg {
                    name: "target".into(),
                    value: crate::candidate::Value::Text("@s".into()),
                    raw: "@s".into(),
                    index: 0,
                },
                InvocationArg {
                    name: "x".into(),
                    value: crate::candidate::Value::Float(100.0),
                    raw: "100".into(),
                    index: 1,
                },
            ],
        };
        let mut column = 0;
        let _ = Target::from_invocation(&invocation, &mut column).unwrap();
        let error = Triple::from_invocation(&invocation, &mut column).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("需要 3 列"), "{message}");
    }

    #[test]
    fn numbers_are_range_checked() {
        let invocation = registry().resolve("give sword 300").unwrap();
        let mut column = 1;
        let error = u8::from_invocation(&invocation, &mut column).unwrap_err();
        assert!(error.to_string().contains("u8"), "{error}");
    }

    #[test]
    fn returns_map_onto_outcomes() {
        assert_eq!(().into_outcome(), Ok(None));
        assert_eq!("hi".into_outcome(), Ok(Some("hi".into())));
        assert_eq!(
            Result::<(), &str>::Err("boom").into_outcome(),
            Err(CommandError::Execution("boom".into()))
        );
        assert_eq!(
            Result::<String, &str>::Ok("ok".into()).into_outcome(),
            Ok(Some("ok".into()))
        );
    }

    #[test]
    fn builtin_specs_describe_themselves() {
        let specs = <u32 as CommandParam>::param_specs("count");
        assert_eq!(specs.len(), <u32 as CommandParam>::ARITY);
        assert_eq!(specs[0].hint, ValueHint::Integer);

        let specs = <Option<Triple> as CommandParam>::param_specs("position");
        assert_eq!(specs.len(), 3);
        assert!(specs.iter().all(|spec| spec.optional));
        assert_eq!(specs[0].name, "position.x");

        let specs = <bool as CommandParam>::param_specs("on");
        assert_eq!(specs[0].source.own_candidates().len(), 2);
    }

    #[test]
    fn glam_vectors_flatten_by_component() {
        let specs = <glam::Vec3 as CommandParam>::param_specs("position");
        assert_eq!(<glam::Vec3 as CommandParam>::ARITY, 3);
        assert_eq!(
            specs
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            ["position.x", "position.y", "position.z"]
        );
        assert_eq!(specs[0].hint, ValueHint::Float);
        assert!(specs.iter().all(|spec| !spec.optional));

        let specs = <glam::Vec2 as CommandParam>::param_specs("uv");
        assert_eq!(
            specs
                .iter()
                .map(|spec| spec.name.as_str())
                .collect::<Vec<_>>(),
            ["uv.x", "uv.y"]
        );

        let specs = <glam::Vec4 as CommandParam>::param_specs("color");
        assert_eq!(specs.len(), 4);
        assert_eq!(specs[3].name, "color.w");
    }

    #[test]
    fn glam_vectors_are_read_back_from_their_columns() {
        let mut registry = CommandRegistry::new();
        registry.register(
            CommandSpec::new("move")
                .param(ParamSpec::free("to.x", ValueHint::Float))
                .param(ParamSpec::free("to.y", ValueHint::Float))
                .param(ParamSpec::free("to.z", ValueHint::Float)),
        );
        let invocation = registry.resolve("move 1 2.5 -3").unwrap();
        let mut column = 0;
        assert_eq!(
            glam::Vec3::from_invocation(&invocation, &mut column).unwrap(),
            glam::Vec3::new(1.0, 2.5, -3.0)
        );
        assert_eq!(column, 3);
    }

    #[test]
    fn integer_vectors_parse_each_column_as_an_integer() {
        let mut registry = CommandRegistry::new();
        registry.register(
            CommandSpec::new("plot")
                .param(ParamSpec::free("cell.x", ValueHint::Integer))
                .param(ParamSpec::free("cell.y", ValueHint::Integer)),
        );
        let invocation = registry.resolve("plot -2 7").unwrap();
        let mut column = 0;
        assert_eq!(
            glam::IVec2::from_invocation(&invocation, &mut column).unwrap(),
            glam::IVec2::new(-2, 7)
        );

        let specs = <glam::IVec3 as CommandParam>::param_specs("cell");
        assert_eq!(specs[0].hint, ValueHint::Integer);

        // 小数在"解析"这一步就被整数列拦住了（还没轮到向量去收）。
        assert!(registry.resolve("plot 1.5 2").is_err());
    }

    #[test]
    fn optional_glam_vectors_can_be_omitted_as_a_whole() {
        let mut registry = CommandRegistry::new();
        registry.register(
            CommandSpec::new("spawn")
                .param(ParamSpec::free("at.x", ValueHint::Float).optional())
                .param(ParamSpec::free("at.y", ValueHint::Float).optional())
                .param(ParamSpec::free("at.z", ValueHint::Float).optional()),
        );
        let invocation = registry.resolve("spawn").unwrap();
        let mut column = 0;
        assert_eq!(
            <Option<glam::Vec3> as CommandParam>::from_invocation(&invocation, &mut column)
                .unwrap(),
            None
        );
        assert_eq!(column, 0);
    }
}
