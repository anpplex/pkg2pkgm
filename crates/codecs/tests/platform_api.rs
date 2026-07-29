#[test]
fn rejects_the_wrong_public_constructor_for_this_host() {
    let tests = trybuild::TestCases::new();

    #[cfg(not(windows))]
    tests.compile_fail("tests/ui/native_constructor_non_windows.rs");

    #[cfg(windows)]
    tests.compile_fail("tests/ui/wine_constructor_windows.rs");
}
