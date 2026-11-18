use ims_mem2::Library;
use ims_mem2_proj::{cell_out, expand_cols, ne_name, Cell};

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let mut lib = Library::new();
    for p in &files {
        lib.import_path(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    }
    lib.finish();
    for ne in &lib.nes {
        let Some(t) = ne.table_by_name("RT") else { continue };
        let nm = ne_name(&lib, ne);
        let cols = expand_cols(t);
        for row in 0..t.len() as u32 {
            if !t.is_alive(row) { continue }
            let mut s = nm.clone();
            for c in cols.iter() {
                s.push('|');
                match cell_out(&lib, "RT", t, row, c, None) {
                    Cell::Null => s.push('-'),
                    Cell::Int(v) => s.push_str(&v.to_string()),
                    Cell::Text(x) => s.push_str(&x),
                }
            }
            println!("{s}");
        }
    }
}
