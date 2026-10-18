use trycmd::TestCases;

#[test]
fn cli_trycmd() {
    let t = TestCases::new();
    t.case("tests/cmd/*.trycmd");
}
