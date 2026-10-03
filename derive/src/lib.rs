//! `gamecmd` 的 proc-macro：`#[command]`。
//!
//! 标在一个函数上，按它的签名生成一整个**同名模块**（模块在类型命名空间、函数在值
//! 命名空间，可以同名）：
//!
//! ```ignore
//! #[command("tp", "把角色传送到一个坐标", alias = "teleport")]
//! fn tp(player: PlayerRef, position: Vec3) -> Result<String, String> { .. }
//!
//! // 生成：
//! mod tp {
//!     pub fn spec() -> gamecmd::CommandSpec { .. }        // 注册用：平铺好的参数列
//!     pub fn args(invocation: &gamecmd::Invocation) -> Result<(PlayerRef, Vec3), gamecmd::CommandError> { .. }
//!     pub fn run(invocation: &gamecmd::Invocation) -> Result<Option<String>, gamecmd::CommandError> { .. }
//!     pub fn entry() -> gamecmd::CommandSpec { .. }        // 只有纯命令才生成
//! }
//! ```
//!
//! 于是注册是 `registry.register(tp::spec())` 或者 `gamecmd::register_commands!(registry; tp)`，
//! 拆包是 `tp::args(invocation)?`。**生成的名字就是 `snake_case` 的函数名**，不做任何
//! 大小写变换。
//!
//! # 上下文参数
//!
//! 如果第一个参数是 `&mut T`（比如 `state: &mut GameState`），它**不占命令参数列**，
//! 而是原样透传：`run` 的签名变成 `run(invocation, state: &mut T)`，由调用方拿着自己的
//! 状态调它。没有这种参数时，生成的 `entry()` 会把 `run` 直接挂到 `CommandSpec::run`
//! 上，变成一条"纯命令"（见 `gamecmd::CommandFn`）。
//!
//! 宏不知道 `T` 是什么，也不依赖任何引擎 —— 它只是把那个类型原样抄进 `run` 的签名。

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
    Error, FnArg, Ident, ItemFn, LitStr, Pat, Token, Type,
};

/// `#[command]` 的属性参数。
#[derive(Default)]
struct CommandArgs {
    name: Option<String>,
    description: Option<String>,
    aliases: Vec<String>,
}

impl Parse for CommandArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = CommandArgs::default();
        let mut positional = 0;
        while !input.is_empty() {
            if input.peek(LitStr) {
                let literal: LitStr = input.parse()?;
                match positional {
                    0 => args.name = Some(literal.value()),
                    1 => args.description = Some(literal.value()),
                    _ => {
                        return Err(Error::new(
                            literal.span(),
                            "`#[command]` 最多两个位置参数：命令名、说明",
                        ))
                    }
                }
                positional += 1;
            } else {
                let key: Ident = input.parse()?;
                input.parse::<Token![=]>()?;
                let value: LitStr = input.parse()?;
                match key.to_string().as_str() {
                    "name" => args.name = Some(value.value()),
                    "desc" | "description" => args.description = Some(value.value()),
                    "alias" => args.aliases.push(value.value()),
                    other => {
                        return Err(Error::new(
                            key.span(),
                            format!(
                                "未知的 `#[command]` 参数 `{other}`（支持 name / desc / alias）"
                            ),
                        ))
                    }
                }
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(args)
    }
}

/// 按函数签名生成命令的注册 / 拆包 / 执行代码。
///
/// 三种写法：
///
/// ```ignore
/// #[command]                                                  // 命令名 = 函数名
/// #[command("give", "往背包里加物品", alias = "g")]            // 名字 + 说明 + 别名
/// #[command(name = "give", description = "…", alias = "g")]
/// ```
///
/// 每个非上下文参数的类型都要实现 `gamecmd::CommandParam`（基础类型和 glam 向量
/// `gamecmd` 已经实现好，自己定义的类型在自己的 crate 里实现）。
#[proc_macro_attribute]
pub fn command(attributes: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attributes as CommandArgs);
    let function = parse_macro_input!(item as ItemFn);
    match expand(&args, &function) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(args: &CommandArgs, function: &ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &function.sig.ident;
    let visibility = &function.vis;
    let command_name = args.name.clone().unwrap_or_else(|| fn_name.to_string());

    // --- 拆形参：开头的 `&mut T` 是宿主上下文，其余都是命令参数列 ---
    let mut inputs = function.sig.inputs.iter();
    let mut context: Option<(&Pat, &Type)> = None;
    if let Some(first) = inputs.clone().next() {
        if let FnArg::Receiver(receiver) = first {
            return Err(Error::new(
                receiver.span(),
                "`#[command]` 只能标在自由函数上，不支持方法（没有 `self`）",
            ));
        }
        if let FnArg::Typed(typed) = first {
            if let Type::Reference(reference) = &*typed.ty {
                if reference.mutability.is_some() {
                    context = Some((&typed.pat, &typed.ty));
                    inputs.next();
                }
            }
        }
    }

    let mut idents: Vec<&Ident> = Vec::new();
    let mut types: Vec<&Type> = Vec::new();
    for input in inputs {
        let FnArg::Typed(typed) = input else {
            return Err(Error::new(input.span(), "`#[command]` 不支持 `self` 参数"));
        };
        let Pat::Ident(pattern) = &*typed.pat else {
            return Err(Error::new(
                typed.pat.span(),
                "`#[command]` 的参数必须是简单名字（不支持解构）",
            ));
        };
        idents.push(&pattern.ident);
        types.push(&typed.ty);
    }

    let describe = args.description.as_ref().map(|description| {
        quote! { spec = spec.describe(#description); }
    });
    let aliases = args.aliases.iter().map(|alias| {
        quote! { spec = spec.alias(#alias); }
    });
    let params = idents.iter().zip(&types).map(|(ident, ty)| {
        // 参数名要以**字符串**传给 `param_specs`（它拼的是 `position.x` 这种列名），
        // 而调用 / 解构那边要的是同一个标识符 —— 所以这里用 `stringify!` 取字面量。
        quote! {
            for param in <#ty as ::gamecmd::CommandParam>::param_specs(::std::stringify!(#ident)) {
                spec = spec.param(param);
            }
        }
    });

    let destructure = quote! { let ( #(#idents,)* ) = args(invocation)?; };

    let run = match context {
        Some((pat, ty)) => quote! {
            /// 执行体：拆参数 → 调原函数。开头的上下文由宿主原样传进来。
            pub fn run(
                invocation: &::gamecmd::Invocation,
                #pat: #ty,
            ) -> ::std::result::Result<
                ::std::option::Option<::std::string::String>,
                ::gamecmd::CommandError,
            > {
                #destructure
                ::gamecmd::CommandReturn::into_outcome(super::#fn_name(#pat, #(#idents),*))
            }
        },
        None => quote! {
            /// 执行体：拆参数 → 调原函数。签名就是 `gamecmd::CommandFn`。
            pub fn run(
                invocation: &::gamecmd::Invocation,
            ) -> ::std::result::Result<
                ::std::option::Option<::std::string::String>,
                ::gamecmd::CommandError,
            > {
                #destructure
                ::gamecmd::CommandReturn::into_outcome(super::#fn_name(#(#idents),*))
            }
        },
    };

    // 纯命令才自动挂执行体；带上下文的命令由宿主自己接。
    let entry = context.is_none().then(|| {
        quote! {
            /// 注册入口：一条自带的纯命令（`spec` + `run`）。
            pub fn entry() -> ::gamecmd::CommandSpec {
                let mut spec = spec();
                spec.run = ::std::option::Option::Some(run);
                spec
            }
        }
    });

    Ok(quote! {
        #function

        /// `#[command]` 为上面这个函数生成的注册 / 拆包 / 执行入口。
        ///
        /// 模块名和函数名相同（模块在类型命名空间、函数在值命名空间，不冲突），
        /// 所以读起来是 `give::spec()` / `give::run(..)`。
        #visibility mod #fn_name {
            #[allow(unused_imports)]
            use super::*;

            /// 命令的形状：每个形参按它的 `CommandParam` 平铺成参数列。
            pub fn spec() -> ::gamecmd::CommandSpec {
                let mut spec = ::gamecmd::CommandSpec::new(#command_name);
                #describe
                #(#aliases)*
                #(#params)*
                spec
            }

            /// 把一次调用拆成形参类型（元组的顺序就是形参顺序）。
            #[allow(unused_mut, unused_variables, clippy::let_unit_value)]
            pub fn args(
                invocation: &::gamecmd::Invocation,
            ) -> ::std::result::Result<( #(#types,)* ), ::gamecmd::CommandError> {
                let mut column = 0usize;
                #(
                    let #idents = <#types as ::gamecmd::CommandParam>::from_invocation(
                        invocation,
                        &mut column,
                    )?;
                )*
                ::std::result::Result::Ok(( #(#idents,)* ))
            }

            #run
            #entry
        }
    })
}
