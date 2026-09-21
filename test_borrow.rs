struct Foo { idx: usize }
fn main() {
    let mut out: Vec<Foo> = Vec::new();
    out.push(Foo { idx: out.len() });
}
