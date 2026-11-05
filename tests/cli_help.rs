use trycmd::TestCases;

#[test]
#[allow(clippy::expect_used)]
fn cli_trycmd() {
    let t = TestCases::new();
    t.insert_var("[VERSION]", env!("CARGO_PKG_VERSION"))
        .expect("insert version var");
    t.case("tests/cmd/*.trycmd");
}
