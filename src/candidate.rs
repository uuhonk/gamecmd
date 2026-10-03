//! 候选值和值类型：命令名、参数值在补全列表里的统一表示。

/// 参数值的类型提示。
///
/// 它有两个用途：在没有候选值（[`ParamSource::Free`](crate::ParamSource::Free)）时
/// 告诉界面"这里该填什么"，以及在 [`ParamSpec::parse`](crate::ParamSpec::parse)
/// 时决定把文本转成哪种 [`Value`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ValueHint {
    /// 不限定：原样当文本。
    #[default]
    Any,
    /// 一个单词的标识符 / 名字。
    Text,
    /// 可以带空格、需要引号的文本（`"player one"`）。
    QuotedText,
    /// 十进制整数（`-3`）。
    Integer,
    /// 浮点数（`1.5`）。
    Float,
    /// `true` / `false`。
    Bool,
}

impl ValueHint {
    /// 界面上的中文短标签。
    pub fn label(self) -> &'static str {
        match self {
            ValueHint::Any => "值",
            ValueHint::Text => "文本",
            ValueHint::QuotedText => "\"文本\"",
            ValueHint::Integer => "整数",
            ValueHint::Float => "小数",
            ValueHint::Bool => "true / false",
        }
    }
}

/// 从输入串里解析出来的一个参数值。
///
/// 只有 `Free`（按 [`ValueHint`] 解析）和候选值（原样当文本）两种来源，所以它是
/// 一个很小的封闭集合 —— 命令的执行侧（第二阶段）按它分发即可。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// 文本值（命令名 / 标识符 / 任意字符串）。
    Text(String),
    /// 整数。
    Integer(i64),
    /// 浮点数。
    Float(f64),
    /// 布尔。
    Bool(bool),
}

impl Value {
    /// 取出文本值；不是文本时为 `None`。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    /// 取出整数；数值型会做一次收窄转换。
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Integer(value) => Some(*value),
            Value::Float(value) => Some(*value as i64),
            _ => None,
        }
    }

    /// 取出浮点数。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(value) => Some(*value),
            Value::Integer(value) => Some(*value as f64),
            _ => None,
        }
    }

    /// 取出布尔值。
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Text(text) => f.write_str(text),
            Value::Integer(value) => write!(f, "{value}"),
            Value::Float(value) => write!(f, "{value}"),
            Value::Bool(value) => write!(f, "{value}"),
        }
    }
}

/// 一个候选值：命令名、别名、字面量分支、或者参数值，补全列表里都用它。
///
/// `value` 是**真正会被写进输入串**的东西（不带引号、不带空格），`label` 只是显示
/// 用的别名（默认等于 `value`），`tooltip` 是选中 / 悬停时的说明。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Candidate {
    /// 补全时写进输入串的值。
    pub value: String,
    /// 界面显示的文字；`None` 表示直接用 `value`。
    pub label: Option<String>,
    /// 说明文字。
    pub tooltip: Option<String>,
}

impl Candidate {
    /// 一个只有值的候选。
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: None,
            tooltip: None,
        }
    }

    /// 值 + 显示文字（比如枚举值 `creative` 显示成 `创造模式`）。
    pub fn labeled(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: Some(label.into()),
            tooltip: None,
        }
    }

    /// 附上说明文字。
    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// 界面应该显示的文字。
    pub fn display(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.value)
    }
}

impl From<&str> for Candidate {
    fn from(value: &str) -> Self {
        Candidate::new(value)
    }
}

impl From<String> for Candidate {
    fn from(value: String) -> Self {
        Candidate::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labeled_candidate_displays_its_label_but_completes_to_the_value() {
        let candidate = Candidate::labeled("creative", "创造模式");
        assert_eq!(candidate.display(), "创造模式");
        assert_eq!(candidate.value, "creative");
    }

    #[test]
    fn values_convert_between_number_representations() {
        assert_eq!(Value::Integer(3).as_f64(), Some(3.0));
        assert_eq!(Value::Float(2.5).as_i64(), Some(2));
        assert_eq!(Value::Text("x".into()).as_i64(), None);
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
    }
}
