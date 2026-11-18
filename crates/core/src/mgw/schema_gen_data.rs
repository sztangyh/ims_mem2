// MGW 表声明（`tools/port_mgw_schema.py` 生成，**勿手改**）。
//
// 来源：`C:/dev/ims_ws/data/mgw/moc_schema.json` + `moc_key.json`（键 / 位图）。
// dedicated MOC 一张表（有 key），single 层无键（`KEYED=false`，每网元一行）。
// 只给 **cols 非空** 的 MOC 建表：16 个 single MOC 的 cols 为空（参数全在 defaults 里），
// ims_core 那边也没有它们的 `_cur` 表（只进 `mgw_ne_config`）。
// 全部列都是 `Text`：MGW 值在 ims_core 里一律 TEXT，且原文无逗号/引号，可逐字节还原。

use crate::mml_table;
use crate::table::TableApi;

mml_table! {
    ATONE, Atone,
    key [bt, bn, toneid],
    bt: Text,
    bn: Text,
    toneid: Text,
}

mml_table! {
    BFD, Bfd,
    key [srcip, dstip],
    bn: Text,
    ifn: Text,
    bfdnm: Text,
    srcip: Text,
    dstip: Text,
    disclo: Text,
    discre: Text,
}

mml_table! {
    BRD, Brd,
    key [r#fn, sn, bp],
    r#fn: Text,
    sn: Text,
    bp: Text,
    bt: Text,
    hbt: Text,
    bs: Text,
    bn: Text,
}

mml_table! {
    CALLDIAG, Calldiag,
    rprctrl: Text,
}

mml_table! {
    CLKSRC, Clksrc,
    line1pri: Text,
    line2pri: Text,
}

mml_table! {
    CODECCAP, Codeccap,
    bn: Text,
    codec: Text,
}

mml_table! {
    DSPCODECCAP, Dspcodeccap,
    key [bn, dspno],
    bn: Text,
    dspno: Text,
}

mml_table! {
    FRM, Frm,
    key [r#fn],
    r#fn: Text,
    loc: Text,
}

mml_table! {
    FRMARC, Frmarc,
    r#fn: Text,
}

mml_table! {
    GWADDR, Gwaddr,
    key [ipaddr],
    bn: Text,
    ipaddr: Text,
    gwip: Text,
}

mml_table! {
    H248LNK, H248lnk,
    key [linkid],
    linkid: Text,
    vmgwid: Text,
    mgcidx: Text,
    tt: Text,
    localip: Text,
    localport: Text,
    peerip: Text,
    linkname: Text,
    sn: Text,
}

mml_table! {
    IPADDR, Ipaddr,
    key [ipaddr],
    bt: Text,
    bn: Text,
    ift: Text,
    ifn: Text,
    ipaddr: Text,
    mask: Text,
}

mml_table! {
    IPIF, Ipif,
    key [bt, bn, ifn],
    bt: Text,
    bn: Text,
    ifn: Text,
    enable: Text,
    desp: Text,
    ifclose: Text,
}

mml_table! {
    L2UALKS, L2ualks,
    key [lks],
    protype: Text,
    lksname: Text,
    lks: Text,
}

mml_table! {
    L2UALNK, L2ualnk,
    key [protype, lnkno],
    protype: Text,
    bn: Text,
    lnkno: Text,
    lnkname: Text,
    lks: Text,
    localpn: Text,
    localip1: Text,
    remotepn: Text,
    remoteip1: Text,
    prio: Text,
}

mml_table! {
    M3DE, M3de,
    key [dex],
    dex: Text,
    den: Text,
    dpc: Text,
    lex: Text,
}

mml_table! {
    M3LE, M3le,
    key [lex],
    lex: Text,
    len: Text,
    opc: Text,
}

mml_table! {
    M3LKS, M3lks,
    key [lsx],
    lsx: Text,
    lsn: Text,
    adx: Text,
    lse: Text,
}

mml_table! {
    M3LNK, M3lnk,
    key [lnk],
    lnk: Text,
    bn: Text,
    lkn: Text,
    lip1: Text,
    lp: Text,
    rip1: Text,
    rp: Text,
    lsx: Text,
    pno: Text,
}

mml_table! {
    M3RT, M3rt,
    key [lsx, dex],
    rn: Text,
    dex: Text,
    lsx: Text,
}

mml_table! {
    MGC, Mgc,
    key [vmgwid, mgcidx],
    vmgwid: Text,
    mgcidx: Text,
    mid: Text,
    mss: Text,
}

mml_table! {
    MTP2LNK, Mtp2lnk,
    key [lnkno],
    lnkno: Text,
    lnkname: Text,
    ifbn: Text,
    opn: Text,
    e1t1n: Text,
    strtts: Text,
    endts: Text,
    spfbn: Text,
    subbn: Text,
    subbcid: Text,
    lnktype: Text,
    lks: Text,
    binifid: Text,
}

mml_table! {
    N7DSP, N7dsp,
    key [index],
    index: Text,
    name: Text,
    dpc: Text,
    ospindex: Text,
}

mml_table! {
    N7LKS, N7lks,
    key [index],
    index: Text,
    name: Text,
    dspidx: Text,
}

mml_table! {
    N7LNK, N7lnk,
    key [index],
    index: Text,
    name: Text,
    lksidx: Text,
    slc: Text,
    sndslc: Text,
    mtp2no: Text,
}

mml_table! {
    N7RT, N7rt,
    key [index],
    index: Text,
    name: Text,
    lksidx: Text,
    dspidx: Text,
}

mml_table! {
    OFI, Ofi,
    key [index],
    name: Text,
    natopc: Text,
    index: Text,
}

mml_table! {
    Q921LNK, Q921lnk,
    key [lnkno],
    lnkno: Text,
    lnkname: Text,
    lks: Text,
    binifid: Text,
    ifbn: Text,
    opn: Text,
    e1t1n: Text,
    spfbn: Text,
    subbn: Text,
    subbcid: Text,
}

mml_table! {
    ROUTE, Route,
    key [bt, dstip, dstmask],
    bt: Text,
    bn: Text,
    dstip: Text,
    dstmask: Text,
    nexthop: Text,
}

mml_table! {
    RSVPORT, Rsvport,
    key [ipaddr],
    bn: Text,
    ipaddr: Text,
}

mml_table! {
    RTBAK, Rtbak,
    key [ipaddr],
    bn: Text,
    ifn: Text,
    ipaddr: Text,
    ipaddrbak: Text,
}

mml_table! {
    S2LFRM, S2lfrm,
    key [bn, pn, srtcn],
    bn: Text,
    pn: Text,
    srtcn: Text,
    endcn: Text,
    frame: Text,
}

mml_table! {
    S2LPORT, S2lport,
    key [bn, pn],
    bn: Text,
    pn: Text,
}

mml_table! {
    SFP, Sfp,
    key [id],
    id: Text,
    value: Text,
}

mml_table! {
    SPFAIDE, Spfaide,
    key [spfbn, aidbn],
    spfbn: Text,
    aidbn: Text,
}

mml_table! {
    SPFSUBRD, Spfsubrd,
    key [spfbn, subbn],
    spfbn: Text,
    subbn: Text,
}

mml_table! {
    SUBBRD, Subbrd,
    key [r#fn, sn, subbn],
    r#fn: Text,
    sn: Text,
    subbn: Text,
    bn: Text,
}

mml_table! {
    TDMIU, Tdmiu,
    key [bn, opt, tidfv],
    bn: Text,
    opt: Text,
    tidfv: Text,
    tidlv: Text,
    vmgwid: Text,
    rt: Text,
    hostid: Text,
}

mml_table! {
    TIMESYC, Timesyc,
    info: Text,
}

mml_table! {
    TRADDR, Traddr,
    key [bt, bn, ifn],
    bt: Text,
    bn: Text,
    ifn: Text,
    traddr: Text,
}

mml_table! {
    V5E1, V5e1,
    key [e1id],
    e1id: Text,
    lks: Text,
    e1lnkid: Text,
    ifbn: Text,
    opn: Text,
    e1n: Text,
}

mml_table! {
    V5LNK, V5lnk,
    key [lnkno],
    lnkno: Text,
    lnkname: Text,
    e1id: Text,
    spfbn: Text,
    subbn: Text,
    subbcid: Text,
}

mml_table! {
    VMGW, Vmgw,
    key [vmgwid],
    vmgwid: Text,
    mid: Text,
}

/// 全部 MOC（**排序**，`gen_moc_index` 用二分）。
pub static MGW_MOCS: &[&str] = &[
    "ATONE",
    "BFD",
    "BRD",
    "CALLDIAG",
    "CLKSRC",
    "CODECCAP",
    "DSPCODECCAP",
    "FRM",
    "FRMARC",
    "GWADDR",
    "H248LNK",
    "IPADDR",
    "IPIF",
    "L2UALKS",
    "L2UALNK",
    "M3DE",
    "M3LE",
    "M3LKS",
    "M3LNK",
    "M3RT",
    "MGC",
    "MTP2LNK",
    "N7DSP",
    "N7LKS",
    "N7LNK",
    "N7RT",
    "OFI",
    "Q921LNK",
    "ROUTE",
    "RSVPORT",
    "RTBAK",
    "S2LFRM",
    "S2LPORT",
    "SFP",
    "SPFAIDE",
    "SPFSUBRD",
    "SUBBRD",
    "TDMIU",
    "TIMESYC",
    "TRADDR",
    "V5E1",
    "V5LNK",
    "VMGW",
];

/// 每个 MOC 的层：`0` = dedicated（有键）/ `1` = single（每网元一条）。
pub static MGW_LAYER: &[u8] = &[0, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];

/// 每个 MOC 里需要转"整数位掩码"的参数（`NAME-0&NAME-1` → 十进制掩码）。
pub static MGW_BITMAP: &[&[&str]] = &[
    &[],
    &[],
    &[],
    &["RPRCTRL"],
    &[],
    &["CODEC"],
    &["CODEC", "DSPNO"],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
    &[],
];

/// 全局常量参数（`mgw_default`）：`(moc, param, value)`。
pub static MGW_DEFAULTS: &[(&str, &str, &str)] = &[
    ("BFD", "BT", "HRB"),
    ("BFD", "IFT", "GE"),
    ("BFD", "TYPE", "IF"),
    ("BRD", "MEMT", "93"),
    ("CALLDIAG", "MINNUM", "300"),
    ("CALLDIAG", "MINRT", "10"),
    ("CALLDIAG", "START", "ENABLE"),
    ("CCDIGITMAP", "DIGITMAP", "x."),
    ("CLK", "BRDTYPE", "CLK"),
    ("CLK", "CLKMODE", "SOURCE"),
    ("CLK", "CTRL", "NO"),
    ("CLK", "GRADE", "THREE"),
    ("CLK", "MODE", "AUTO"),
    ("CLK", "TYPE", "EXT2MHZ"),
    ("CLKSRC", "BRDTYPE", "CLK"),
    ("CLKSRC", "EXT1PRI", "FIRST"),
    ("CLKSRC", "EXTSSM", "UNKNOWN"),
    ("CLKSRC", "FSSM", "FORCE"),
    ("CLKSRC", "GPSPRI", "FOURTH"),
    ("CLKSRC", "GPSTYPE", "GPS"),
    ("CLKSRC", "SLOT", "SA4"),
    ("CLKSRC", "SRCTYPE", "EXT2MBit"),
    ("CODECCAP", "PUBCODEC", "YES"),
    ("CODECCAP", "TSTRSCNUM", "0"),
    ("CODECCAP", "TSTRSCSWH", "OFF"),
    ("CODECCAP", "WBLTT", "NB"),
    ("DSPCODECCAP", "CODEC", "0"),
    ("DSPCODECCAP", "LCK", "DISABLE"),
    ("DSPCODECCAP", "MODE", "HAND"),
    ("ECNYPARA", "CPUDWNL", "30"),
    ("ECNYPARA", "CPUDWNN", "40"),
    ("ECNYPARA", "CPUUPL", "60"),
    ("ECNYPARA", "CPUUPN", "50"),
    ("ECNYPARA", "ET", "23&59&59"),
    ("ECNYPARA", "ST", "00&00&00"),
    ("ECNYPARA", "START", "DISABLE"),
    ("ECNYPARA", "TCDWNL", "30"),
    ("ECNYPARA", "TCDWNN", "40"),
    ("ECNYPARA", "TCUPL", "60"),
    ("ECNYPARA", "TCUPN", "50"),
    ("ECNYPARA", "TIMES", "3"),
    ("ENGINEID", "ENGINEID", "800007DB01C0A800FD"),
    ("ENVTHD", "ITEMH", "131"),
    ("ENVTHD", "ITEML", "41"),
    ("ENVTHD", "MTRLGY", "IMP"),
    ("ENVTHD", "VOLTH", "-59"),
    ("ENVTHD", "VOLTL", "-40"),
    ("FLNKTHD", "FEHITHD", "90"),
    ("FLNKTHD", "FELOTHD", "80"),
    ("FLNKTHD", "HITHD", "90"),
    ("FLNKTHD", "LOTHD", "80"),
    ("FRM", "CN", "DIRECT"),
    ("FRM", "FV", "SSM160"),
    ("FRM", "SHF", "0"),
    ("FRMARC", "CABT", "N68_22"),
    ("FRMARC", "CN", "2147483647"),
    ("FRMARC", "FID", "2147483647"),
    ("FRMARC", "LN", "2147483647"),
    ("FRMARC", "NDID", "2147483647"),
    ("GWADDR", "TIMEOUT", "NoAging"),
    ("H248LNK", "BP", "FRONT"),
    ("H248LNK", "FN", "1"),
    ("H248LNK", "PEERPORT", "2944"),
    ("H248LNK", "PTHMODE", "TWOPATH"),
    ("H248LNK", "SBN", "SBN0"),
    ("H248PARA", "CT", "BINARY"),
    ("H248PARA", "CTNW", "60"),
    ("H248PARA", "CTW", "80"),
    ("H248PARA", "HT", "MD5"),
    ("H248PARA", "LT", "8"),
    ("H248PARA", "MAXT", "7"),
    ("H248PARA", "MAXTI", "4000"),
    ("H248PARA", "MCT", "10000"),
    ("H248PARA", "MGCLMT", "10"),
    ("H248PARA", "MGLMT", "10"),
    ("H248PARA", "MINTI", "1000"),
    ("H248PARA", "NTTI", "0"),
    ("H248PARA", "PRTI", "4000"),
    ("H248PARA", "SPI", "256"),
    ("H248PARA", "ST", "NOIMPL"),
    ("H248PARA", "TRF", "OFF"),
    ("H248PARA", "TT", "UDP"),
    ("H248PARA", "VMGWID", "0"),
    ("HDTHD", "NEWTHD", "80"),
    ("IPADDR", "DESP", "信令地址"),
    ("IPIF", "AUTONEGO", "NO"),
    ("IPIF", "BEARBW", "512000"),
    ("IPIF", "DOMAIN", "0"),
    ("IPIF", "IFT", "GE"),
    ("IPIF", "INTERVALTIME", "65535"),
    ("IPIF", "MTU", "1500"),
    ("IPIF", "RTPMUX", "NONE"),
    ("LICSCAN", "INTERVAL", "5"),
    ("M3DE", "DET", "AS"),
    ("M3DE", "NI", "NAT"),
    ("M3LE", "LET", "SG"),
    ("M3LE", "NI", "NAT"),
    ("M3LNK", "ASF", "ACTIVE"),
    ("M3LNK", "BT", "SPF"),
    ("MGC", "ANNEXC", "1"),
    ("MGC", "CONTCTRLASSN", "NO"),
    ("MGC", "DWRAP", "NO"),
    ("MGC", "H248VER", "V1"),
    ("MGC", "MIDTYPE", "IP"),
    ("MGC", "OUTADA", "1"),
    ("MGC", "PERMANENTREQID", "0"),
    ("MGC", "PRONEGO", "NO"),
    ("MGC", "STREAMMODE", "Inactive"),
    ("MRPARA", "AMU", "A"),
    ("MRPARA", "CAMELV", "-12"),
    ("MRPARA", "CAST", "80"),
    ("MRPARA", "CASV", "-15"),
    ("MRPARA", "CIDDTL", "700"),
    ("MRPARA", "CIDM", "BELLCORE"),
    ("MRPARA", "DDT", "10"),
    ("MRPARA", "DMM", "LONG_MATCH"),
    ("MRPARA", "DTMFV", "-7"),
    ("MRPARA", "DTOFF", "80"),
    ("MRPARA", "DTON", "100"),
    ("MRPARA", "DTS", "TDM_SIDE_ENABLE"),
    ("MRPARA", "FSKV", "-13"),
    ("MRPARA", "LONGT", "4"),
    ("MRPARA", "LSWH", "DISABLE"),
    ("MRPARA", "MFCV", "-12"),
    ("MRPARA", "MT", "10"),
    ("MRPARA", "RDT", "180"),
    ("MRPARA", "SAST", "400"),
    ("MRPARA", "SASV", "-20"),
    ("MRPARA", "SHORTT", "4"),
    ("MRPARA", "STLEN", "16"),
    ("MRPARA", "TTYV", "-10"),
    ("MTP2LNK", "IFBT", "S4L"),
    ("N7DSP", "NI", "NAT"),
    ("N7LNK", "TESTCODE", "170"),
    ("OFI", "INTRESVLD", "NO"),
    ("OFI", "INTVLD", "NO"),
    ("OFI", "NATLEN", "LABEL24"),
    ("OFI", "NATRESVLD", "NO"),
    ("OFI", "NATVLD", "YES"),
    ("OFI", "RESTARTFLAG", "NO"),
    ("OFI", "SERACH0", "NAT"),
    ("OFI", "SPFLAG", "NO"),
    ("OFI", "STPFLAG", "YES"),
    ("PWDPOLICY", "AUTOLOCKTIME", "30"),
    ("PWDPOLICY", "AUTOLOCKUNUSER", "NO"),
    ("PWDPOLICY", "AUTOLOCKUSER", "YES"),
    ("PWDPOLICY", "CHKUNITS", "5"),
    ("PWDPOLICY", "COMPLICACY", "15"),
    ("PWDPOLICY", "CPFL", "TRUE"),
    ("PWDPOLICY", "CUETIME", "5"),
    ("PWDPOLICY", "MAXEFFTIME", "90"),
    ("PWDPOLICY", "MAXMISSTIMES", "3"),
    ("PWDPOLICY", "MONITORTIME", "5"),
    ("PWDPOLICY", "PWDMAXLEN", "32"),
    ("PWDPOLICY", "PWDMINLEN", "8"),
    ("PWDPOLICY", "RESTRICTMODPWD", "YES"),
    ("PWDPOLICY", "UDBL", "180"),
    ("Q921LNK", "IFBT", "S4L"),
    ("Q921LNK", "NETUSER", "NET"),
    ("Q921LNK", "TS", "16"),
    ("RCVFUN", "CTRLLEVEL", "2"),
    ("RCVFUN", "FAULTBRD", "5"),
    ("RCVFUN", "START", "ENABLE"),
    ("ROUTE", "RTTYPE", "NEXTHOP"),
    ("RSVPORT", "EPORT", "3785"),
    ("RSVPORT", "SPORT", "3784"),
    ("RTBAK", "IFT", "GE"),
    ("S2LFRM", "BT", "S4L"),
    ("S2LPORT", "BT", "S4L"),
    ("S2LPORT", "FT", "SDH"),
    ("S2LPORT", "MODE", "HUAWEI"),
    ("S2LPORT", "TYPE", "E1"),
    ("SFP", "MODTYPE", "VALUE"),
    ("SPC", "CD", "DDIR"),
    ("SPC", "CT", "TDM_TDM"),
    ("SPC", "DIAD", "0"),
    ("SPC", "DISA", "0.0.0.0"),
    ("SPC", "DOPN", "3"),
    ("SPC", "DPN", "45"),
    ("SPC", "DRFN", "0"),
    ("SPC", "DRPN", "0"),
    ("SPC", "DRSN", "0"),
    ("SPC", "DTFN", "1"),
    ("SPC", "DTS", "1"),
    ("SPC", "DTSN", "15"),
    ("SPC", "ID", "0"),
    ("SPC", "SIAD", "0"),
    ("SPC", "SISA", "0.0.0.0"),
    ("SPC", "SOPN", "1"),
    ("SPC", "SPCN", "梅林动力监控-1"),
    ("SPC", "SPN", "1"),
    ("SPC", "SRFN", "0"),
    ("SPC", "SRPN", "0"),
    ("SPC", "SRSN", "0"),
    ("SPC", "STFN", "1"),
    ("SPC", "STS", "1"),
    ("SPC", "STSN", "14"),
    ("SPFSUBRD", "CM", "PRO_PRA"),
    ("SUBBRD", "BS", "ONEBACKUP"),
    ("SUBBRD", "BT", "CMU"),
    ("SYSINFO", "CFGFTP", "NO"),
    ("T1MODE", "MODE", "SEC"),
    ("TDMIU", "BT", "S4L"),
    ("TRADDR", "SN", "255"),
    ("TSTPARA", "BSC", "2"),
    ("TSTPARA", "CONNECTMODE", "ENABLE"),
    ("TSTPARA", "ESC", "8"),
    ("TSTPARA", "ETIME", "04&00&00"),
    ("TSTPARA", "LOOPTST", "DISABLE"),
    ("TSTPARA", "PRBS", "225"),
    ("TSTPARA", "SCLOG", "PART"),
    ("TSTPARA", "SPARETST", "DISABLE"),
    ("TSTPARA", "START", "ENABLE"),
    ("TSTPARA", "STARTFORTDM", "ENABLE"),
    ("TSTPARA", "STIME", "01&00&00"),
    ("V5E1", "IFBT", "S4L"),
    ("V5LNK", "TS", "16"),
    ("VMGW", "ATMNUM", "221184"),
    ("VMGW", "AUTOSWP", "YES"),
    ("VMGW", "CISTT", "1000"),
    ("VMGW", "CODEC", "G.711A"),
    ("VMGW", "IPNUM", "86016"),
    ("VMGW", "LNKFAILLEN", "30"),
    ("VMGW", "LNKHBTIME", "3"),
    ("VMGW", "LNKMAXHBLOSS", "30"),
    ("VMGW", "MASTERMGCDETECTFLAG", "NO"),
    ("VMGW", "MASTERMGCDETECTTIME", "5"),
    ("VMGW", "MIDTYPE", "IP"),
    ("VMGW", "MWDMODE", "STATIC"),
    ("VMGW", "MWDVAL", "0"),
    ("VMGW", "NETTYPE", "WCDMA"),
    ("VMGW", "NONROOTLENGTH", "8"),
    ("VMGW", "RLSINTV", "30"),
    ("VMGW", "ROOTLENGTH", "8"),
    ("VMGW", "RPINTV", "3"),
    ("VMGW", "RPTIMES", "3"),
    ("VMGW", "TDMNUM", "400384"),
];

/// MOC 名（大小写不敏感）→ 表下标。
pub fn gen_moc_index(name: &[u8]) -> Option<usize> {
    if name.is_empty() || name.len() > 16 {
        return None;
    }
    let mut buf = [0u8; 16];
    for (i, &c) in name.iter().enumerate() {
        buf[i] = c.to_ascii_uppercase();
    }
    MGW_MOCS.binary_search_by(|m| m.as_bytes().cmp(&buf[..name.len()])).ok()
}

/// 按下标建一张空表。
pub fn new_gen_table(id: usize) -> Option<Box<dyn TableApi>> {
    Some(match id {
        0 => Box::new(AtoneTable::new()),
        1 => Box::new(BfdTable::new()),
        2 => Box::new(BrdTable::new()),
        3 => Box::new(CalldiagTable::new()),
        4 => Box::new(ClksrcTable::new()),
        5 => Box::new(CodeccapTable::new()),
        6 => Box::new(DspcodeccapTable::new()),
        7 => Box::new(FrmTable::new()),
        8 => Box::new(FrmarcTable::new()),
        9 => Box::new(GwaddrTable::new()),
        10 => Box::new(H248lnkTable::new()),
        11 => Box::new(IpaddrTable::new()),
        12 => Box::new(IpifTable::new()),
        13 => Box::new(L2ualksTable::new()),
        14 => Box::new(L2ualnkTable::new()),
        15 => Box::new(M3deTable::new()),
        16 => Box::new(M3leTable::new()),
        17 => Box::new(M3lksTable::new()),
        18 => Box::new(M3lnkTable::new()),
        19 => Box::new(M3rtTable::new()),
        20 => Box::new(MgcTable::new()),
        21 => Box::new(Mtp2lnkTable::new()),
        22 => Box::new(N7dspTable::new()),
        23 => Box::new(N7lksTable::new()),
        24 => Box::new(N7lnkTable::new()),
        25 => Box::new(N7rtTable::new()),
        26 => Box::new(OfiTable::new()),
        27 => Box::new(Q921lnkTable::new()),
        28 => Box::new(RouteTable::new()),
        29 => Box::new(RsvportTable::new()),
        30 => Box::new(RtbakTable::new()),
        31 => Box::new(S2lfrmTable::new()),
        32 => Box::new(S2lportTable::new()),
        33 => Box::new(SfpTable::new()),
        34 => Box::new(SpfaideTable::new()),
        35 => Box::new(SpfsubrdTable::new()),
        36 => Box::new(SubbrdTable::new()),
        37 => Box::new(TdmiuTable::new()),
        38 => Box::new(TimesycTable::new()),
        39 => Box::new(TraddrTable::new()),
        40 => Box::new(V5e1Table::new()),
        41 => Box::new(V5lnkTable::new()),
        42 => Box::new(VmgwTable::new()),
        _ => return None,
    })
}
