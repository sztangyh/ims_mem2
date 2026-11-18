//! `ims_tui`：fzf 式交互查询客户端（纯 UI；数据全部来自 `ims_svc`）。
//!
//! - **首页（列表）**：输入栏在**底部**、无边框；候选富展示（原字冠+角色+网元+行数+落地）；
//!   每键击即查（字冠池 = 对称剥冠 + 核心别名，服务端完成）；**ESC 直接退出**。
//! - **详情页**（Enter 进入）：左侧配置条目 + 右侧详情（边框/颜色）；**Tab 切网元**、
//!   **PageUp/PageDown 翻页**、**↑↓ 选左侧条目**、**ESC 返回列表**。
//!
//! ```powershell
//! ims_svc 127.0.0.1:7711 ..\agcf_data\ALLME_2026*.txt   # 先起服务端
//! ims_tui --tcp 127.0.0.1:7711                          # 客户端
//! ims_tui --tcp 127.0.0.1:7711 --selftest               # 无终端自检（对拍用）
//! ```

mod proto;
mod transport;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use proto::{ObjView, Row};
use transport::Transport;

const POOLS: [&str; 3] = ["prefix", "subscriber", "trunk"];
const POOL_CN: [&str; 3] = ["字冠", "用户", "中继"];
const NE_NAMES: [&str; 6] = ["AGCF51", "AGCF52", "AGCF63", "AGCF64", "AGCF65", "AGCF66"];
const PAGE: usize = 200;
/// 客户端对象缓存容量（LRU；适配详情翻页 + 历史查询复用，不反复下载）
const CACHE: usize = 300;
/// 历史查询条目上限（F8 查看）
const HIST: usize = 50;

#[derive(PartialEq, Clone, Copy)]
enum View {
    List,
    Detail,
    History,
}

/// 一次历史查询（F8 显示，选择后恢复）
struct History {
    pool: &'static str,
    kind: &'static str,
    input: String,
    query: String,
    total: usize,
    keys: Vec<String>,
}

struct App {
    tp: Transport,
    pool_idx: usize,
    input: String,
    kind: &'static str,
    query: String,
    rows: Vec<Row>,
    total: usize,
    sel: usize,
    obj: Option<ObjView>,
    view: View,
    // 详情页状态
    detail_ne: usize, // 0=全部网元，1..=6 对应 AGCF51..66
    detail_sel: usize,
    detail_scroll: u16,
    // 对象缓存（LRU：io|ne|key → 对象）
    cache: Vec<(String, ObjView)>,
    // 历史查询（F8）
    history: Vec<History>,
    hist_sel: usize,
    msg: String,
    last_ms: f64,
}

fn candidates(pool: &str) -> &'static [&'static str] {
    match pool {
        "subscriber" => &["Subscriber"],
        "trunk" => &["Trunk", "PraLink", "N7tg", "Tgsrt"],
        _ => &[],
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 角色 → 中文短标（首页行里的彩色单字）
fn role_tag(r: &str) -> (&'static str, Color) {
    match r {
        "called" => ("被", Color::Magenta),
        "caller" => ("主", Color::Cyan),
        "transform" => ("变", Color::Green),
        "subscriber" => ("户", Color::Yellow),
        "trunk" => ("继", Color::Blue),
        "dn" => ("缺", Color::LightRed),
        _ => ("?", Color::DarkGray),
    }
}

/// NE 位掩码数组 → "AGCF=51|52|63"（序号取"AGCF"后的数字）
fn nes_badge(nes: &[i64]) -> String {
    if nes.is_empty() {
        "-".into()
    } else {
        let nums: Vec<&str> = nes
            .iter()
            .filter_map(|&n| NE_NAMES.get((n as usize).saturating_sub(1)))
            .map(|s| &s[4..])
            .collect();
        format!("AGCF={}", nums.join("|"))
    }
}

impl App {
    fn new(tp: Transport) -> App {
        App {
            tp,
            pool_idx: 0,
            input: String::new(),
            kind: "prefix",
            query: String::new(),
            rows: Vec::new(),
            total: 0,
            sel: 0,
            obj: None,
            view: View::List,
            detail_ne: 0,
            detail_sel: 0,
            detail_scroll: 0,
            cache: Vec::new(),
            history: Vec::new(),
            hist_sel: 0,
            msg: String::new(),
            last_ms: 0.0,
        }
    }

    fn pool(&self) -> &'static str {
        POOLS[self.pool_idx]
    }

    /// fzf：每次输入变化即查（同步）
    fn search(&mut self) {
        if self.input.is_empty() {
            self.rows.clear();
            self.total = 0;
            self.query.clear();
            self.sel = 0;
            self.msg = format!("池 {} · 输入即查（Enter 详情）", POOL_CN[self.pool_idx]);
            return;
        }
        let pool = self.pool();
        let req = format!(
            r#"{{"q":"catalog","pool":"{pool}","kind":"{}","arg":"{}","limit":{}}}"#,
            self.kind,
            esc(&self.input),
            PAGE
        );
        let t0 = std::time::Instant::now();
        let resp = self.tp.call(&req);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        match proto::parse_catalog(&resp) {
            Ok(p) => {
                let pool = self.pool();
                let kind = self.kind;
                let input = self.input.clone();
                let q = p.query.clone();
                let total = p.total;
                let keys: Vec<String> = p.rows.iter().take(6).map(|r| r.key.clone()).collect();
                self.total = p.total;
                self.rows = p.rows;
                self.query = p.query;
                self.sel = 0;
                self.last_ms = ms;
                self.note_history(pool, kind, &input, &q, total, &keys);
            }
            Err(e) => self.msg = e,
        }
    }

    /// 记录一条历史查询（F8 查看；同 池+输入 去重，保留最近）
    fn note_history(
        &mut self,
        pool: &'static str,
        kind: &'static str,
        input: &str,
        query: &str,
        total: usize,
        keys: &[String],
    ) {
        self.history
            .retain(|h| !(h.pool == pool && h.kind == kind && h.input == input));
        self.history.insert(
            0,
            History {
                pool,
                kind,
                input: input.to_string(),
                query: query.to_string(),
                total,
                keys: keys.to_vec(),
            },
        );
        self.history.truncate(HIST);
    }

    /// 按 对象标识 缓存拉取：命中返回缓存副本，未命中下载并入 LRU 缓存。
    fn fetch_obj(&mut self, io: &str, ne: i64, key: &str, req: &str) -> Option<ObjView> {
        let ck = format!("{io}|{ne}|{key}");
        if let Some(pos) = self.cache.iter().position(|(k, _)| k == &ck) {
            let o = self.cache.remove(pos).1;
            self.cache.push((ck, o.clone()));
            return Some(o);
        }
        match proto::parse_object(&self.tp.call(req)) {
            Ok(o) => {
                let c = o.clone();
                self.cache.push((ck, c));
                if self.cache.len() > CACHE {
                    self.cache.remove(0);
                }
                Some(o)
            }
            Err(_) => None,
        }
    }

    /// Enter：进详情页。字冠 → PrefixProfile；其它池 → 候选 IO 逐个探；结果走缓存
    fn open(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let pool = self.pool();
        let key = self.rows[self.sel].key.clone();
        let ne = self.rows[self.sel].nes.first().copied().unwrap_or(1);
        let mut target: Option<(String, i64, String)> = None;
        if pool == "prefix" {
            target = Some(("PrefixProfile".to_string(), 1, key.clone()));
        } else {
            for io in candidates(pool) {
                let req = format!(
                    r#"{{"q":"object","io":"{io}","ne":{ne},"key":"{}"}}"#,
                    esc(&key)
                );
                if proto::parse_object(&self.tp.call(&req)).is_ok() {
                    target = Some((io.to_string(), ne, key.clone()));
                    break;
                }
            }
        }
        let Some((io, ne, key)) = target else {
            self.msg = format!("key={key}：候选 IO 都没命中");
            return;
        };
        let req = format!(
            r#"{{"q":"object","io":"{io}","ne":{ne},"key":"{}"}}"#,
            esc(&key)
        );
        match self.fetch_obj(&io, ne, &key, &req) {
            Some(o) => {
                self.obj = Some(o);
                self.view = View::Detail;
                self.detail_ne = 0;
                self.detail_sel = 0;
                self.detail_scroll = 0;
            }
            None => self.msg = format!("{io} {key} 读取失败"),
        }
    }

    /// 详情页左侧条目（按 detail_ne 过滤）：返回 (字段下标, 标签, 是否段标题)
    fn detail_entries(&self) -> Vec<(usize, String, bool)> {
        let Some(o) = &self.obj else { return Vec::new() };
        let ne = if self.detail_ne == 0 {
            None
        } else {
            Some(NE_NAMES[self.detail_ne - 1])
        };
        o.fields
            .iter()
            .enumerate()
            .filter(|(_, (k, _, _))| match ne {
                None => true,
                Some(n) => k.contains(n),
            })
            .map(|(i, (k, v, _))| (i, k.clone(), v.is_none()))
            .collect()
    }
}

/// 自检打印用：`名称=值（中文名）`；中文名缺失就只打 `名称=值`。
fn fmt_field(name: &str, val: Option<&str>, cn: Option<&str>) -> String {
    match (val, cn) {
        (Some(v), Some(c)) if !c.is_empty() => format!("{name}={v}({c})"),
        (Some(v), _) => format!("{name}={v}"),
        (None, Some(c)) if !c.is_empty() => format!("{name}({c})=null"),
        (None, _) => format!("{name}=null"),
    }
}

fn run(term: &mut ratatui::DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    loop {
        term.draw(|f| draw(f, app))?;
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match app.view {
                View::List => match k.code {
                    // 首页 ESC 直接退出（用户要求）；q 也退（习惯保留）
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Char('q') if app.input.is_empty() => return Ok(()),
                    KeyCode::Tab => {
                        app.pool_idx = (app.pool_idx + 1) % POOLS.len();
                        app.kind = "prefix";
                        app.search();
                    }
                    KeyCode::Char('t') => {
                        app.kind = if app.kind == "prefix" { "tail" } else { "prefix" };
                        app.search();
                    }
                    KeyCode::Backspace if !app.input.is_empty() => {
                        app.input.pop();
                        app.search();
                    }
                    KeyCode::Char(c) => {
                        app.input.push(c);
                        app.search();
                    }
                    KeyCode::Enter => app.open(),
                    KeyCode::F(8) => {
                        app.view = View::History;
                        app.hist_sel = 0;
                    }
                    KeyCode::Up => app.sel = app.sel.saturating_sub(1),
                    KeyCode::Down if app.sel + 1 < app.rows.len() => app.sel += 1,
                    _ => {}
                },
                View::Detail => match k.code {
                    KeyCode::Esc => app.view = View::List,
                    KeyCode::Tab => {
                        let total = 1 + NE_NAMES.len();
                        app.detail_ne = (app.detail_ne + 1) % total;
                        app.detail_sel = 0;
                        app.detail_scroll = 0;
                    }
                    KeyCode::PageUp => app.detail_scroll = app.detail_scroll.saturating_sub(1),
                    KeyCode::PageDown => app.detail_scroll = app.detail_scroll.saturating_add(1),
                    KeyCode::Up => app.detail_sel = app.detail_sel.saturating_sub(1),
                    KeyCode::Down => {
                        let n = app.detail_entries().len();
                        if app.detail_sel + 1 < n {
                            app.detail_sel += 1;
                        }
                    }
                    _ => {}
                },
                View::History => match k.code {
                    KeyCode::Esc => app.view = View::List,
                    KeyCode::Up => app.hist_sel = app.hist_sel.saturating_sub(1),
                    KeyCode::Down => {
                        if app.hist_sel + 1 < app.history.len() {
                            app.hist_sel += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(h) = app.history.get(app.hist_sel) {
                            app.pool_idx = POOLS.iter().position(|p| *p == h.pool).unwrap_or(0);
                            app.kind = h.kind;
                            app.input = h.input.clone();
                            app.search();
                            app.view = View::List;
                        }
                    }
                    _ => {}
                },
            }
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    match app.view {
        View::List => draw_list(f, app),
        View::Detail => draw_detail(f, app),
        View::History => draw_history(f, app),
    }
}

/// 历史查询（F8）：显示历史条目，Enter 恢复、Esc 返回
fn draw_history(f: &mut Frame, app: &App) {
    let items: Vec<ListItem> = app
        .history
        .iter()
        .map(|h| {
            let pi = POOLS.iter().position(|p| *p == h.pool).unwrap_or(0);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", POOL_CN[pi]),
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ),
                Span::raw(format!(
                    " {}  → 查询\"{}\" 匹配 {}",
                    h.input, h.query, h.total
                )),
                Span::styled(
                    format!("  {}", h.keys.join(" · ")),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let mut st = ListState::default();
    st.select(if items.is_empty() { None } else { Some(app.hist_sel) });
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" 历史查询 {} 条（Enter 恢复 · Esc 返回） ", app.history.len())),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        f.area(),
        &mut st,
    );
}

/// 首页：底部输入 + 无边框列表（fzf 风格）
fn draw_list(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(2), Constraint::Length(1)])
        .split(f.area());

    // 状态行（无边框）
    let arrow = if !app.query.is_empty() && app.query != app.input {
        format!(" 查询\"{}\"", app.query)
    } else {
        String::new()
    };
    let head = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", POOL_CN[app.pool_idx]),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!(" 匹配 {} · 显示 {} · {:.1}ms{}", app.total, app.rows.len(), app.last_ms, arrow)),
    ]));
    f.render_widget(head, chunks[0]);

    // 候选列表（无边框）
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| ListItem::new(row_line(i == app.sel, r)))
        .collect();
    let mut st = ListState::default();
    st.select(if items.is_empty() { None } else { Some(app.sel) });
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        chunks[1],
        &mut st,
    );

    // 输入栏（无边框，fzf 风格在底部）
    let inp = Paragraph::new(Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(format!("{}_", app.input)),
    ]));
    f.render_widget(inp, chunks[2]);
}

/// 详情页：左=配置条目，右=详情；Tab 切网元 / Page 翻页 / ↑↓ 选条目 / Esc 返回
fn draw_detail(f: &mut Frame, app: &App) {
    let Some(o) = &app.obj else { return };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(2), Constraint::Length(1)])
        .split(f.area());

    // 标题行：池 · key · 当前网元过滤
    let ne_txt = if app.detail_ne == 0 {
        "全部网元".to_string()
    } else {
        format!("网元 {}", NE_NAMES[app.detail_ne - 1])
    };
    let head = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", o.io),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!("  {}  │ {}  │ Tab切网 · Page翻页 · Esc返回", o.key, ne_txt)),
    ]));
    f.render_widget(head, chunks[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(chunks[1]);

    // 左：配置条目（按网元过滤）
    let entries = app.detail_entries();
    let items: Vec<ListItem> = entries
        .iter()
        .map(|(_, label, _)| ListItem::new(label.clone()))
        .collect();
    let mut lst = ListState::default();
    lst.select(if items.is_empty() { None } else { Some(app.detail_sel) });
    f.render_stateful_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(format!(
                " 配置条目 {}（{}）",
                entries.len(),
                ne_txt
            )))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        body[0],
        &mut lst,
    );

    // 右：详情（选中条目的值）
    let mut lines: Vec<Line> = Vec::new();
    if let Some(&(i, ref label, is_hdr)) = entries.get(app.detail_sel) {
        let (_, v, cn) = &o.fields[i];
        if is_hdr {
            lines.push(Line::from(Span::styled(
                label.clone(),
                Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::from(Span::styled(
            "──────────────────────────",
            Style::default().fg(Color::DarkGray),
        )));
        // 中文名（zh.tsv）：有就单独一行，没有就不占地方
        if let Some(c) = cn.as_deref().filter(|c| !c.is_empty()) {
            lines.push(Line::from(Span::styled(
                format!("中文：{c}"),
                Style::default().fg(Color::LightCyan),
            )));
        }
        // 开关型（Bits）字段：把置位的开关连中文说明列出来（D67 的 S 层）。
        // 值本身太长（32 位 ≈ 300 字），这里只列**置位的**那几位，最多 10 条。
        if let Some((_, items)) = o.bits.iter().find(|(col, _)| col == label) {
            let on: Vec<&(String, bool, Option<String>)> = items.iter().filter(|x| x.1).collect();
            let cn_cnt = items.iter().filter(|x| x.2.is_some()).count();
            lines.push(Line::from(Span::styled(
                format!("开关：共 {} 位 / 置位 {} / 有中文 {}", items.len(), on.len(), cn_cnt),
                Style::default().fg(Color::DarkGray),
            )));
            if on.is_empty() {
                lines.push(Line::from(Span::styled(
                    "（全部关闭）",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            for (n, _, c) in on.iter().take(10) {
                lines.push(Line::from(vec![
                    Span::styled(format!("{n:<10}"), Style::default().fg(Color::Green)),
                    Span::styled(
                        c.as_deref().unwrap_or("（无中文说明）").to_string(),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
        }
        match v {
            None => {
                lines.push(Line::from(Span::styled(
                    "（段标题：该分组下无其它字段）",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            Some(s) => {
                // 值按“=”拆分着色：键绿、值白
                for part in s.split(", ").take(12) {
                    if let Some((k, val)) = part.split_once('=') {
                        lines.push(Line::from(vec![
                            Span::styled(format!("{k:<12}"), Style::default().fg(Color::Green)),
                            Span::styled(val, Style::default().fg(Color::Yellow)),
                        ]));
                    } else {
                        lines.push(Line::from(Span::raw(part.to_string())));
                    }
                }
            }
        }
    } else {
        lines.push(Line::from("（该网元下无配置条目）"));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" 详情 "))
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        body[1],
    );

    // 底部帮助
    let help = Paragraph::new(Line::from(Span::styled(
        " ↑↓=选条目  Tab=切网元  PageUp/Down=翻页  Esc=返回列表 ",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(help, chunks[2]);
}

/// 列表行：`> 7 [被|主]  AGCF=51|52 ×2 ◆落`
fn row_line(sel: bool, r: &Row) -> Line<'static> {
    let mut spans = Vec::new();
    if sel {
        spans.push(Span::styled(
            "> ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(
        r.key.clone(),
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw("  "));
    for part in r.roles.split('/') {
        let (t, c) = role_tag(part);
        spans.push(Span::styled(format!("{t} "), Style::default().fg(c)));
    }
    // 命中键与原文不同 → 核心命中，标暗色核心
    if let Some(mk) = &r.mkey {
        if mk != &r.key {
            spans.push(Span::styled(
                format!(" ⟵{mk}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
    }
    spans.push(Span::styled(
        format!("  {} ×{}", nes_badge(&r.nes), r.hits),
        Style::default().fg(Color::DarkGray),
    ));
    if r.lands == Some(true) {
        spans.push(Span::styled(" ◆落", Style::default().fg(Color::LightYellow)));
    }
    Line::from(spans)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 没给地址时：先读 `IMS_DAT_SVC`，再退默认（见 Transport::resolve_addr）
    let mut addr = String::new();
    let mut selftest = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--tcp" => {
                if args.get(i + 1).map(|a| is_addr(a)).unwrap_or(false) {
                    addr = args[i + 1].clone();
                    i += 1;
                }
            }
            a if is_addr(a) && args.get(i + 1).is_none() => {
                addr = a.to_string();
            }
            "--selftest" => selftest = true,
            _ => {}
        }
        i += 1;
    }

    let addr = Transport::resolve_addr(if addr.is_empty() { None } else { Some(addr.clone()) });
    let tp = match Transport::connect(&addr) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("连 {addr} 失败: {e}（先起服务端：ims_svc {addr} <MML文件...>）");
            std::process::exit(1);
        }
    };
    if selftest {
        selftest_with(tp);
        return;
    }
    let mut app = App::new(tp);
    app.msg = "输入即查 · Enter=详情 · Esc=退出 · Tab=切池".into();
    let mut term = ratatui::init();
    let res = run(&mut term, &mut app);
    ratatui::restore();
    if let Err(e) = res {
        eprintln!("TUI 退出：{e}");
    }
}

fn is_addr(a: &str) -> bool {
    match a.rsplit_once(':') {
        Some((_, port)) => !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// 自检（与 ims_svc --selftest 的输出逐行对拍；耗时行在 check_tui.py 里被剥掉）
fn selftest_with(mut tp: Transport) {
    println!("形态: {}", tp.kind());
    let hello = tp.call(r#"{"q":"hello","v":1}"#);
    println!("hello: {hello}");
    for (pool, mode, arg) in [
        ("prefix", "prefix", "7552"),
        ("prefix", "prefix", "7"),
        ("prefix", "prefix", "0086755"),
        ("subscriber", "prefix", "755212"),
        ("subscriber", "tail", "0001"),
        ("trunk", "prefix", "100"),
    ] {
        let req =
            format!(r#"{{"q":"catalog","pool":"{pool}","kind":"{mode}","arg":"{arg}","limit":5}}"#);
        let t0 = std::time::Instant::now();
        let resp = tp.call(&req);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        match proto::parse_catalog(&resp) {
            Ok(p) => {
                let mut parts = Vec::new();
                for r in p.rows.iter().take(3) {
                    let land = r.lands.map(|l| if l { "落" } else { "" }).unwrap_or("");
                    parts.push(format!("{}[{}{}]", r.key, r.roles, land));
                }
                println!(
                    "  {pool}/{mode} {arg} → 查询\"{}\" 匹配 {}，前 {} 个 [{}] · {:.1} ms（含传输）",
                    p.query,
                    p.total,
                    p.rows.len(),
                    parts.join(" "),
                    ms
                );
            }
            Err(e) => println!("  {pool}/{mode} {arg}: {e}"),
        }
    }

    {
        let resp = tp.call(r#"{"q":"catalog","pool":"prefix","kind":"prefix","arg":"7552","limit":1}"#);
        if let Ok(p) = proto::parse_catalog(&resp) {
            if let Some(r) = p.rows.first() {
                let k = r.key.clone();
                let req = format!(r#"{{"q":"object","io":"PrefixProfile","ne":1,"key":"{}"}}"#, esc(&k));
                if let Ok(o) = proto::parse_object(&tp.call(&req)) {
                    let f: Vec<String> = o
                        .fields
                        .iter()
                        .take(4)
                        .map(|(a, b, c)| fmt_field(a, b.as_deref(), c.as_deref()))
                        .collect();
                    println!("  PrefixProfile({k}): {} · 共 {} 字段", f.join(" | "), o.fields.len());
                }
            }
        }
    }

    let keys = {
        let resp = tp.call(r#"{"q":"catalog","pool":"trunk","kind":"prefix","arg":"1","limit":3}"#);
        proto::parse_catalog(&resp)
            .map(|p| p.rows.iter().map(|r| r.key.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    for k in keys {
        for io in ["Trunk", "PraLink"] {
            let req = format!(r#"{{"q":"object","io":"{io}","ne":1,"key":"{k}"}}"#);
            if let Ok(o) = proto::parse_object(&tp.call(&req)) {
                let f: Vec<String> = o
                    .fields
                    .iter()
                    .take(4)
                    .map(|(a, b, c)| fmt_field(a, b.as_deref(), c.as_deref()))
                    .collect();
                println!("  {io}({}, {k}): {} · 链接 {:?}", o.ne, f.join(" "), o.links);
                if let Some((field, _, _)) = o.links.first() {
                    let req = format!(
                        r#"{{"q":"link","io":"{io}","field":"{field}","ne":{},"key":"{k}"}}"#,
                        o.ne
                    );
                    let t0 = std::time::Instant::now();
                    let r = tp.call(&req);
                    if let Ok(t) = proto::parse_links(&r) {
                        println!(
                            "    {io}.{field} → {} 个目标 {:?} · {:.2} ms",
                            t.len(),
                            t.iter().take(2).collect::<Vec<_>>(),
                            t0.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                }
            }
        }
    }
}

