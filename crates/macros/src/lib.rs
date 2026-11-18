//! `mml_table!` —— 瘦声明 → 行结构 / 解析中间态 / `ColStore` / `TableHooks`。
//!
//! 语法（P0.3 子集）：
//!
//! ```ignore
//! mml_table! {
//!     DNC, Dnc,                  // MOC 名, 类型前缀 → DncRow / DncPending / DncHooks / DncTable
//!     key [dcx],                 // 自然键（可多列、可省）
//!     dcx: U16,                  // 字段（列序 = 声明序）
//!     dct: Enum,                 // (MOC.PARAM) 标签池，1 B
//!     nd:  Text,                 // 原文驻留（Sym）
//!     dcl: U8,
//! }
//! ```
//!
//! P0.3 只覆盖 `U8 / U16 / Text / Enum`（DNC 用到的全部）。
//! `U32 / Pfx / PfxText / Uri / Bits / tmp / agg / branch` 在 P0.4 加，
//! 加之前先补对应的"行内表示 + 版本链表示"，不能半吊子。
//!
//! 生成物与手写参照（`crates/core/src/schema/dnc.rs`）**同构**：
//! 字段都是 `ColVal`（可打包列 `Small`、文本族 `Sym`），所以版本链/`as_of` 直接可用。

use proc_macro::TokenStream;
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{bracketed, Ident, Result, Token};

mod kw {
    syn::custom_keyword!(key);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    U8,
    U16,
    /// ⚠ 只用 30 bit 存（`Val32`）：值 ≥ 2^30 会退化为 `Default` 并丢值。
    /// 实测值域都远小于 2^30；超限会退化为 `Default`，值级对拍能立刻发现。
    U32,
    Bool,
    Text,
    Enum,
    /// 参数组 `U16[28]`：行内是 `ColVal::Group(池下标)`，`Pending` 里是定长数组 + 位图（P4）。
    /// 语法 `name: U16[28]` 与链展开在下一阶段接入；存储侧（`ColVal::Group` + `GroupStore`）已就绪。
    #[allow(dead_code)]
    GroupU16(usize),
    #[allow(dead_code)]
    GroupU8(usize),
    /// 字冠 `K'...`：行内只留原文（`ColVal::Sym`），数字形态只在 `Pending` 里
    /// 用于键与比较（`PfxNum` 是 68 bit，塞不进 `Val32`）。`PfxText` 同义。
    Pfx,
    /// A2：宽值类型。行内是 4 B 的**宽值池下标**（`ColVal::Group`），池里放 `u64`。
    /// 只有"渲染回去 == 原文"的值才走 typed，否则 `Pending` 里留原文（`#n_raw`）。
    Ip,
    Eid,
    /// `Bits(n)`：`n` = `BITS_TABLES` 下标（位名表由 `port_schema.py` 从 `switch_bitmaps` 生成）
    Bits(usize),
    Pair,
    Spc,
}

struct Field {
    name: Ident,
    kind: Kind,
}

/// 字段名 → 参数名（剥掉 `r#` 前缀；`r#in` 的参数名是 `IN`）。
/// 列类型标签（投影层据此绑定 SQLite 参数）：`t` 文本 / `n` 数字 / `b` 布尔 / `e` 枚举。
fn kind_tag(k: Kind) -> &'static [u8] {
    match k {
        Kind::Text | Kind::Pfx => b"t",
        Kind::Bool => b"b",
        Kind::Enum => b"e",
        Kind::GroupU16(_) | Kind::GroupU8(_) => b"g",
        Kind::Ip => b"i",
        Kind::Eid => b"d",
        Kind::Bits(_) => b"x",
        Kind::Pair => b"p",
        Kind::Spc => b"c",
        Kind::U8 | Kind::U16 | Kind::U32 => b"n",
    }
}


fn param_name(f: &Field) -> String {
    let s = f.name.to_string();
    s.strip_prefix("r#").unwrap_or(&s).to_string()
}

struct TableInput {
    moc: Ident,
    ty: Ident,
    keys: Vec<Ident>,
    fields: Vec<Field>,
}

impl Parse for TableInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let moc: Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let ty: Ident = input.parse()?;
        input.parse::<Token![,]>()?;

        let mut keys = Vec::new();
        if input.peek(kw::key) {
            input.parse::<kw::key>()?;
            let content;
            bracketed!(content in input);
            let list: Punctuated<Ident, Token![,]> =
                content.parse_terminated(Ident::parse, Token![,])?;
            keys = list.into_iter().collect();
            input.parse::<Token![,]>()?;
        }

        let mut fields = Vec::new();
        while !input.is_empty() {
            let name: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let kind_ident: Ident = input.parse()?;
            // 参数组：`name: U16[28]` / `U8[32]`
            let group_width = if input.peek(syn::token::Bracket) {
                let content;
                syn::bracketed!(content in input);
                let lit: syn::LitInt = content.parse()?;
                Some(lit.base10_parse::<usize>()?)
            } else {
                None
            };

            let kind = match kind_ident.to_string().as_str() {
                "U8" => Kind::U8,
                "U16" => Kind::U16,
                "U32" => Kind::U32,
                "Bool" => Kind::Bool,
                "Text" => Kind::Text,
                "Enum" => Kind::Enum,
                "Pfx" | "PfxText" => Kind::Pfx,
                "Ip" => Kind::Ip,
                "Eid" => Kind::Eid,
                "Pair" => Kind::Pair,
                "Spc" => Kind::Spc,
                "Bits" => {
                    // 语法：`name: Bits(3),` —— 3 = BITS_TABLES 下标
                    let content;
                    syn::parenthesized!(content in input);
                    let lit: syn::LitInt = content.parse()?;
                    Kind::Bits(lit.base10_parse::<usize>()?)
                }
                other => {
                    return Err(syn::Error::new(
                        kind_ident.span(),
                        format!(
                            "未知字段类型 `{other}`（P0.3 支持 U8/U16/U32/Bool/Text/Enum，Pfx/Uri/Bits 在 P0.4）"
                        ),
                    ))
                }
            };
            let kind = match (group_width, kind) {
                (Some(n), Kind::U16) => Kind::GroupU16(n),
                (Some(n), Kind::U8) => Kind::GroupU8(n),
                (Some(_), _) => {
                    return Err(syn::Error::new(name.span(), "参数组只支持 U16[n] / U8[n]"))
                }
                (None, k) => k,
            };
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
            fields.push(Field { name, kind });
        }

        Ok(TableInput {
            moc,
            ty,
            keys,
            fields,
        })
    }
}

#[proc_macro]
pub fn mml_table(input: TokenStream) -> TokenStream {
    let t = syn::parse_macro_input!(input as TableInput);
    match expand(t) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// 字段的 Rust 类型（Pending 里是 `Option<#ty>`）。
fn rust_ty(k: Kind) -> TokenStream2 {
    match k {
        Kind::U8 => quote!(u8),
        Kind::U16 => quote!(u16),
        Kind::U32 => quote!(u32),
        Kind::Bool => quote!(bool),
        Kind::Text => quote!(crate::sym::Sym),
        Kind::Enum => quote!(crate::label::Label),
        // 行内只存原文；数字形态在 Pending 的 `#n_num`
        Kind::Pfx => quote!(crate::sym::Sym),
        // 参数组：Pending 里是定长数组（元素统一用 u16）
        Kind::GroupU16(_) | Kind::GroupU8(_) => quote!(crate::vhist::GroupVal),
        // A2 宽值：Pending 里统一 u64（Ip 用低 32 bit、Eid = ip<<16|port、Pair = a<<32|b）
        Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc => quote!(u128),
    }
}

/// 把 `v`（Pending 里的值）变成行内 `ColVal`。
fn to_colval(k: Kind) -> TokenStream2 {
    match k {
        Kind::U8 | Kind::U16 => {
            quote!(crate::vhist::ColVal::small(crate::val::Val32::set_masked(v as u32)))
        }
        // 超 30 bit 会退化为 Default（见 Kind::U32 的注释）
        Kind::U32 => quote!(crate::vhist::ColVal::small(
            crate::val::Val32::set(v as u32).unwrap_or_default()
        )),
        Kind::Bool => quote!(crate::vhist::ColVal::small(crate::val::Val32::pack(v))),
        Kind::Text => quote!(crate::vhist::ColVal::sym(Some(v))),
        Kind::Pfx => quote!(crate::vhist::ColVal::sym(Some(v))),
        Kind::Enum => quote!(crate::vhist::ColVal::small(crate::val::Val32::set_masked(
            v.as_u8() as u32
        ))),
        // 参数组：行内由 `apply_groups` 写（语法接入见下一阶段）
        Kind::GroupU16(_) | Kind::GroupU8(_) => quote!(crate::vhist::ColVal::DEFAULT),
        // A2 宽值：行内由 `apply_wide` / `merge_wide` 写（要宽值池）
        Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc => {
            quote!(crate::vhist::ColVal::DEFAULT)
        }
    }
}

/// 解析一行里的一个参数（写在 `if k.eq_ignore_ascii_case(b"X") { ... }` 里）。
fn parse_arm(f: &Field, enum_slot: Option<usize>, name_lit: &str) -> TokenStream2 {
    let n = &f.name;
    let _ = name_lit;
    match f.kind {
        Kind::U8 => quote!(p.#n = crate::frombytes::parse_u8(v);),
        Kind::U16 => quote!(p.#n = crate::frombytes::parse_u16(v);),
        // 超 30 bit 的值不再静默退 Default：解析期显式丢弃 + 计 `fallbacks`（D52 例外计数）。
        Kind::U32 => quote!({
            match crate::frombytes::parse_u32(v) {
                Some(x) if x <= crate::val::Val32::MAX => p.#n = Some(x),
                Some(_) => p.fallbacks += 1,
                None => {}
            }
        }),
        Kind::Bool => quote!(p.#n = crate::frombytes::parse_bool(v);),
        Kind::Text => quote!(p.#n = Some(it.intern(crate::frombytes::unquote(v)));),
        Kind::GroupU16(_) | Kind::GroupU8(_) => quote!(),
        // A2 宽值：能逐字节还原的走 typed，否则原文兜底并计 `fallbacks`
        Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc => {
            let raw = format_ident!("{}_raw", f.name);
            let parse = match f.kind {
                Kind::Ip => quote!(crate::frombytes::parse_ip(raw).map(|x| x as u128)),
                Kind::Eid => quote!(crate::frombytes::parse_eid(raw).map(|x| x as u128)),
                Kind::Pair => quote!(crate::frombytes::parse_pair(raw).map(|x| x as u128)),
                Kind::Spc => quote!(crate::frombytes::parse_spc(raw).map(|x| x as u128)),
                Kind::Bits(i) => {
                    let idx = Literal::usize_unsuffixed(i);
                    quote!(crate::frombytes::parse_bits(raw, crate::schema_gen::BITS_TABLES[#idx]))
                }
                _ => unreachable!(),
            };
            let render = match f.kind {
                Kind::Ip => quote!(crate::frombytes::render_ip(n as u32)),
                Kind::Eid => quote!(crate::frombytes::render_eid(n as u64)),
                Kind::Pair => quote!(crate::frombytes::render_pair(n as u64)),
                Kind::Spc => quote!(crate::frombytes::render_spc(n as u32)),
                Kind::Bits(i) => {
                    let idx = Literal::usize_unsuffixed(i);
                    quote!(crate::frombytes::render_bits(n, crate::schema_gen::BITS_TABLES[#idx]))
                }
                _ => unreachable!(),
            };
            quote!({
                let raw = crate::frombytes::unquote(v);
                match #parse {
                    Some(n) if #render.as_bytes() == raw => p.#n = Some(n),
                    _ => {
                        p.#raw = Some(it.intern(raw));
                        p.wide_fb += 1;
                    }
                }
            })
        }
        Kind::Pfx => {
            let num = format_ident!("{}_num", f.name);
            quote!({
                p.#n = Some(it.intern(crate::frombytes::unquote(v)));
                p.#num = crate::frombytes::parse_pfx(v);
            })
        }
        Kind::Enum => {
            let slot = Literal::usize_unsuffixed(enum_slot.unwrap());
            quote!({
                let s = it.intern(crate::frombytes::unquote(v));
                match pools.label_of(ids[#slot], s) {
                    Some(l) => p.#n = Some(l),
                    None => p.fallbacks += 1,
                }
            })
        }
    }
}

/// 类型化读取器（导出/对账/测试用）。
fn getter(f: &Field) -> TokenStream2 {
    let n = &f.name;
    match f.kind {
        Kind::U8 => quote!(pub fn #n(&self) -> Option<u8> { self.#n.as_u32().map(|v| v as u8) }),
        Kind::U16 => quote!(pub fn #n(&self) -> Option<u16> { self.#n.as_u32().map(|v| v as u16) }),
        Kind::U32 => quote!(pub fn #n(&self) -> Option<u32> { self.#n.as_u32() }),
        Kind::Bool => quote!(pub fn #n(&self) -> Option<bool> { self.#n.as_bool() }),
        Kind::Text => quote!(pub fn #n(&self) -> Option<crate::sym::Sym> { self.#n.as_sym() }),
        Kind::Pfx => quote!(pub fn #n(&self) -> Option<crate::sym::Sym> { self.#n.as_sym() }),
        Kind::GroupU16(_) | Kind::GroupU8(_) => quote!(
            pub fn #n(&self) -> Option<u32> { self.#n.as_group() }
        ),
        // A2 宽值：返回**宽值池下标**；真值用 `Table::wide_of(row, col)`，渲染用 `wide_text`
        Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc => quote!(
            pub fn #n(&self) -> Option<u32> { self.#n.as_group() }
        ),
        Kind::Enum => quote!(
            pub fn #n(&self) -> Option<crate::label::Label> {
                self.#n
                    .as_u32()
                    .and_then(|v| u8::try_from(v).ok())
                    .and_then(crate::label::Label::from_u8_0)
            }
        ),
    }
}

fn expand(t: TableInput) -> Result<TokenStream2> {
    let moc = &t.moc;
    let ty = &t.ty;
    let moc_str = moc.to_string();
    let n = t.fields.len();
    if n == 0 {
        return Err(syn::Error::new(moc.span(), "至少要声明一个字段"));
    }
    if n > 1024 {
        return Err(syn::Error::new(
            moc.span(),
            "单表列数上限 1024（列号是 u16；超过这个数说明声明有问题）",
        ));
    }

    // key 必须都是已声明字段
    for k in &t.keys {
        if !t.fields.iter().any(|f| f.name == *k) {
            return Err(syn::Error::new(
                k.span(),
                format!("key 里的 `{k}` 不是已声明字段"),
            ));
        }
    }

    let row = format_ident!("{}Row", ty);
    let pending = format_ident!("{}Pending", ty);
    let hooks = format_ident!("{}Hooks", ty);
    let table = format_ident!("{}Table", ty);

    let mut field_names = Vec::new();      // Pending 的字段（只有声明里的）
    let mut row_fields = Vec::new();      // Row 的字段（声明 + A3 注入的伴生列）
    let mut pending_tys = Vec::new();
    let mut key_tys = Vec::new();
    let mut extra_names = Vec::new();
    let mut extra_tys = Vec::new();
    let mut getters = Vec::new();
    let mut build_fields = Vec::new();
    let mut merge_blocks = Vec::new();
    let mut get_arms = Vec::new();
    let mut set_arms = Vec::new();
    let mut pool_lits = Vec::new();
    let mut group_stmts: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut wide_stmts: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut wide_merge: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut bits_meta: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut bits_arms: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut group_meta: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut decl_lits: Vec<proc_macro2::TokenStream> = Vec::new();
    for f in &t.fields {
        let lit = Literal::byte_string(param_name(f).to_uppercase().as_bytes());
        decl_lits.push(quote!(#lit));
    }
    let mut col_names = Vec::new();
    let mut key_cols: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut col_kinds = Vec::new();
    let mut enum_param_names = Vec::new();
    let mut enum_slots = std::collections::HashMap::new();
    // 列号计数器：`Pfx` 列会**注入一个伴生列** `<NAME>_NUM`（A3），所以列号不等于字段下标
    let mut col: u16 = 0;
    let mut field_col: std::collections::HashMap<String, u16> = std::collections::HashMap::new();

    for f in t.fields.iter() {
        let fname = &f.name;
        // 宽值列（A2）：Ip/Eid/Bits/Pair/Spc
        let wide = matches!(
            f.kind,
            Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc
        );
        let lit = Literal::u16_suffixed(col);
        field_names.push(fname.clone());
        row_fields.push(fname.clone());
        field_col.insert(f.name.to_string(), col);
        col += 1;
        if matches!(f.kind, Kind::GroupU16(_) | Kind::GroupU8(_)) {
            if let Kind::GroupU16(n) | Kind::GroupU8(n) = f.kind {
                let w = Literal::u8_suffixed(n as u8);
                group_meta.push(quote!((#lit, #w)));
            }
            group_stmts.push(quote! {
                if let Some(v) = p.#fname.as_ref() {
                    crate::vhist::ColStore::set_col(
                        row,
                        #lit,
                        crate::vhist::ColVal::group(g.intern(v.clone())),
                    );
                }
            });
        }
        col_names.push(Literal::string(&param_name(f).to_uppercase()));
        col_kinds.push(Literal::u8_suffixed(kind_tag(f.kind)[0]));
        pending_tys.push(rust_ty(f.kind));
        if f.kind == Kind::Pfx {
            let num = format_ident!("{}_num", f.name);
            extra_names.push(num.clone());
            extra_tys.push(quote!(crate::frombytes::PfxNum));
            key_tys.push(quote!(crate::frombytes::PfxNum));
            // ---- A3：注入伴生列 `<NAME>_NUM`（行内 4 B 的 `PfxStore` 下标）----
            let clit = Literal::u16_suffixed(col);
            let cname = Literal::string(&format!("{}_NUM", param_name(f).to_uppercase()));
            row_fields.push(num.clone());
            col_names.push(cname);
            col_kinds.push(Literal::u8_suffixed(b'N'));
            get_arms.push(quote!(#clit => self.#num));
            set_arms.push(quote!(#clit => self.#num = v));
            build_fields.push(quote! { #num: crate::vhist::ColVal::DEFAULT });
            wide_stmts.push(quote! {
                if let Some(n) = p.#num {
                    crate::vhist::ColStore::set_col(
                        row,
                        #clit,
                        crate::vhist::ColVal::group(s.intern_pfx(n)),
                    );
                }
            });
            wide_merge.push(quote! {
                if let Some(n) = p.#num {
                    let nv = crate::vhist::ColVal::group(s.intern_pfx(n));
                    if nv != row.#num {
                        out.push((#clit, row.#num));
                        row.#num = nv;
                    }
                }
            });
            col += 1;
        } else if wide {
            // A2.5：宽值键要能同时表达 typed 与原文兜底
            key_tys.push(quote!(crate::val::WideKey));
        } else {
            key_tys.push(rust_ty(f.kind));
        }
        getters.push(getter(f));

        // ---- A2 宽值：原文兜底字段 + 两个钩子的语句 + 位名表元数据 ----
        if wide {
            let raw = format_ident!("{}_raw", f.name);
            extra_names.push(raw.clone());
            extra_tys.push(quote!(crate::sym::Sym));
            // 建行之后补写（typed 走池、兜底走原文）
            wide_stmts.push(quote! {
                if let Some(v) = p.#fname {
                    crate::vhist::ColStore::set_col(
                        row,
                        #lit,
                        crate::vhist::ColVal::group(s.intern_wide(v)),
                    );
                } else if let Some(s) = p.#raw {
                    crate::vhist::ColStore::set_col(row, #lit, crate::vhist::ColVal::sym(Some(s)));
                }
            });
            // SET/MOD：值变了才写版本
            wide_merge.push(quote! {
                if p.#fname.is_some() || p.#raw.is_some() {
                    let nv = match p.#fname {
                        Some(v) => crate::vhist::ColVal::group(s.intern_wide(v)),
                        None => crate::vhist::ColVal::sym(p.#raw),
                    };
                    if nv != row.#fname {
                        out.push((#lit, row.#fname));
                        row.#fname = nv;
                    }
                }
            });
            if let Kind::Bits(t) = f.kind {
                let ti = Literal::usize_unsuffixed(t);
                bits_meta.push(quote!((#lit, #ti)));
                bits_arms.push(quote!(#lit => Some(crate::schema_gen::BITS_TABLES[#ti])));
            }
        }
        let to_col = to_colval(f.kind);
        if matches!(f.kind, Kind::GroupU16(_) | Kind::GroupU8(_)) {
            // 组列先留空，由 `apply_groups` 驻留后写入
            build_fields.push(quote! { #fname: crate::vhist::ColVal::DEFAULT });
        } else if wide {
            // 宽值列先留空，由 `apply_wide` 驻留后写入
            build_fields.push(quote! { #fname: crate::vhist::ColVal::DEFAULT });
        } else {
            build_fields.push(quote! {
                #fname: p.#fname.map(|v| #to_col).unwrap_or_default()
            });
        }

        // 参数组 / 宽值都不在 `merge` 里处理（要各自的池）
        if !matches!(f.kind, Kind::GroupU16(_) | Kind::GroupU8(_)) && !wide {
            merge_blocks.push(quote! {
                if let Some(v) = p.#fname {
                    let nv = #to_col;
                    if nv != row.#fname {
                        out.push((#lit, row.#fname));
                        row.#fname = nv;
                    }
                }
            });
        }

        get_arms.push(quote!(#lit => self.#fname));
        set_arms.push(quote!(#lit => self.#fname = v));

        if f.kind == Kind::Enum {
            let slot = enum_slots.len();
            enum_slots.insert(f.name.to_string(), slot);
            let pname = format!("{}.{}", moc_str, param_name(f).to_uppercase());
            pool_lits.push(pname.clone());
            enum_param_names.push(pname);
        }
    }

    for k in &t.keys {
        if let Some(c) = field_col.get(&k.to_string()) {
            let lit = Literal::u16_suffixed(*c);
            key_cols.push(quote!(#lit));
        }
    }

    // 解析链：从后往前套 if/else
    let mut chain = quote! {
        {
            let ku = crate::frombytes::unquote(k);
            let vu = crate::frombytes::unquote(v);
            let ko = ku.as_ptr() as usize - body.as_ptr() as usize;
            let vo = vu.as_ptr() as usize - body.as_ptr() as usize;
            p.extras_off.push([ko as u32, ku.len() as u32, vo as u32, vu.len() as u32]);
        }
    };
    for f in t.fields.iter().rev() {
        if let Kind::GroupU16(n) | Kind::GroupU8(n) = f.kind {
            let name = &f.name;
            for i in (1..=n).rev() {
                let lit = Literal::byte_string(
                    format!("{}{}", param_name(f).to_uppercase(), i).as_bytes(),
                );
                let idx = i - 1;
                chain = quote! {
                    if k.eq_ignore_ascii_case(#lit) {
                        let g = p.#name
                            .get_or_insert_with(|| crate::vhist::GroupVal::empty(#n));
                        if let Some(x) = crate::frombytes::parse_u16(v) {
                            g.vals[#idx] = x;
                            g.mask |= 1u32 << #idx;
                        }
                    } else { #chain }
                };
            }
            continue;
        }
        let lit = Literal::byte_string(param_name(f).to_uppercase().as_bytes());
        let slot = enum_slots.get(&f.name.to_string()).copied();
        let arm = parse_arm(f, slot, &param_name(f));
        chain = quote! {
            if k.eq_ignore_ascii_case(#lit) { #arm } else { #chain }
        };
    }

    // 键：类型 + 取 Option 的表达式（Pfx 用 `#n_num`）
    let keyed = !t.keys.is_empty();
    let key_opt = |f: &Field| -> TokenStream2 {
        if f.kind == Kind::Pfx {
            let n = format_ident!("{}_num", f.name);
            quote!(p.#n)
        } else if matches!(
            f.kind,
            Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc
        ) {
            // A2.5：typed → `WideKey::Num`；原文兜底 → `WideKey::Raw`（不掉行）
            let n = &f.name;
            let raw = format_ident!("{}_raw", f.name);
            quote!(p.#n.map(crate::val::WideKey::Num).or_else(|| p.#raw.map(crate::val::WideKey::Raw)))
        } else {
            let n = &f.name;
            quote!(p.#n)
        }
    };
    let (key_ty, key_of_body) = if !keyed {
        (quote!(()), quote!(None))
    } else if t.keys.len() == 1 {
        let k = &t.keys[0];
        let f = t.fields.iter().find(|f| f.name == *k).expect("key 已校验");
        let ty = rust_ty(f.kind);
        let ty = if f.kind == Kind::Pfx {
            quote!(crate::frombytes::PfxNum)
        } else if matches!(
            f.kind,
            Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc
        ) {
            quote!(crate::val::WideKey)
        } else {
            ty
        };
        // 直接返回 `Option<T>`，不要写成 `Some(p.k?)`（clippy 会报多余的 Some/?）
        (ty, key_opt(f))
    } else {
        let mut tys = Vec::new();
        let mut exprs = Vec::new();
        for k in &t.keys {
            let f = t.fields.iter().find(|f| f.name == *k).expect("key 已校验");
            let ty = rust_ty(f.kind);
            tys.push(if f.kind == Kind::Pfx {
                quote!(crate::frombytes::PfxNum)
            } else if matches!(
                f.kind,
                Kind::Ip | Kind::Eid | Kind::Bits(_) | Kind::Pair | Kind::Spc
            ) {
                quote!(crate::val::WideKey)
            } else {
                ty
            });
            exprs.push(key_opt(f));
        }
        (
            quote!((#(#tys),*)),
            quote!(Some((#(#exprs?),*))),
        )
    };

    let _ = enum_param_names;
    let n_lit = Literal::u16_suffixed(col);

    Ok(quote! {
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub struct #row {
            #(pub #row_fields: crate::vhist::ColVal,)*
        }

        impl #row {
            #(#getters)*
        }

        impl crate::vhist::ColStore for #row {
            const COLS: u16 = #n_lit;

            fn get_col(&self, i: u16) -> crate::vhist::ColVal {
                match i {
                    #(#get_arms,)*
                    _ => unreachable!("列号越界"),
                }
            }

            fn set_col(&mut self, i: u16, v: crate::vhist::ColVal) {
                match i {
                    #(#set_arms,)*
                    _ => unreachable!("列号越界"),
                }
            }
        }

        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub struct #pending {
            #(pub #field_names: Option<#pending_tys>,)*
            #(pub #extra_names: Option<#extra_tys>,)*
            /// 枚举池溢出等降级次数（P0.4 起并进分类异常表）
            pub fallbacks: u32,
            /// A2：宽值列"原文兜底"次数（渲染 != 原文；与枚举溢出分开计）
            pub wide_fb: u32,
            /// 未声明参数的原文偏移 `[名偏移, 名长, 值偏移, 值长]`（P1b / D40；不驻留）
            pub extras_off: Vec<[u32; 4]>,
            /// 未声明的参数（参数名, 原文）——提升列用（P1b / D40）
            pub extras: Vec<(crate::sym::Sym, crate::sym::Sym)>,
        }

        pub struct #hooks;

        impl crate::table::TableHooks for #hooks {
            type Row = #row;
            type Pending = #pending;
            type Key = #key_ty;

            const MOC: &'static str = #moc_str;
            const KEYED: bool = #keyed;
            const COL_NAMES: &'static [&'static str] = &[#(#col_names),*];
            const COL_KINDS: &'static [u8] = &[#(#col_kinds),*];
            const KEY_COLS: &'static [u16] = &[#(#key_cols),*];
            const COL_GROUPS: &'static [(u16, u8)] = &[#(#group_meta),*];
            /// A2：`Bits` 列 → `BITS_TABLES` 下标
            const COL_BITS: &'static [(u16, u16)] = &[#(#bits_meta),*];

            fn pools() -> &'static [&'static str] {
                &[#(#pool_lits),*]
            }

            fn apply_groups(
                row: &mut Self::Row,
                p: &Self::Pending,
                g: &mut crate::vhist::GroupStore,
            ) {
                #(#group_stmts)*
            }

            fn apply_wide(
                row: &mut Self::Row,
                p: &Self::Pending,
                s: &crate::vhist::Stores,
            ) {
                #(#wide_stmts)*
            }

            fn merge_wide(
                row: &mut Self::Row,
                p: &Self::Pending,
                s: &crate::vhist::Stores,
            ) -> Vec<(u16, crate::vhist::ColVal)> {
                let mut out: Vec<(u16, crate::vhist::ColVal)> = Vec::new();
                #(#wide_merge)*
                out
            }

            fn fallbacks_of(p: &Self::Pending) -> (u32, u32) {
                (p.wide_fb, p.fallbacks)
            }

            fn bits_names(col: u16) -> Option<&'static [&'static str]> {
                match col {
                    #(#bits_arms,)*
                    _ => None,
                }
            }

            fn take_extras(p: &mut Self::Pending, out: &mut Vec<[u32; 4]>) {
                out.clear();
                out.append(&mut p.extras_off);
            }

            fn parse(
                body: &[u8],
                it: &mut crate::sym::Interner,
                pools: &mut crate::label::LabelCols,
                ids: &[usize],
                p: &mut Self::Pending,
            ) {
                for (k, v) in crate::scan::Params::new(body) {
                    #chain
                }
            }

            fn key_of(p: &Self::Pending) -> Option<Self::Key> {
                #key_of_body
            }

            fn build(p: &Self::Pending, _it: &crate::sym::Interner) -> Self::Row {
                #row { #(#build_fields,)* }
            }

            fn merge(
                row: &mut Self::Row,
                p: &Self::Pending,
                _it: &crate::sym::Interner,
            ) -> Vec<(u16, crate::vhist::ColVal)> {
                let mut out: Vec<(u16, crate::vhist::ColVal)> = Vec::new();
                #(#merge_blocks)*
                out
            }
        }

        pub type #table = crate::table::Table<#hooks>;
    })
}


