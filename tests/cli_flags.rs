#[allow(dead_code, unused_imports)]
mod cli {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs"));
}

use clap::Parser;
use cli::Cli;

type FlagCheck = (Vec<&'static str>, fn(&Cli) -> bool);

#[test]
fn global_flags_accepted_before_subcommand() {
    let cli = Cli::try_parse_from(["bin", "--verbose", "compact"]).unwrap();
    assert!(cli.verbose);
    assert!(matches!(cli.command, cli::Command::Compact));
}

#[test]
fn global_flags_accepted_after_subcommand() {
    let cli = Cli::try_parse_from(["bin", "compact", "--verbose"])
        .inspect_err(|e| panic!("clap rejected flag after subcommand: {e}"));
    assert!(cli.is_ok());
    let cli = cli.unwrap();
    assert!(cli.verbose);
    assert!(matches!(cli.command, cli::Command::Compact));
}

#[test]
fn each_documented_global_flag_works_after_subcommand() {
    let cases: [FlagCheck; 6] = [
        (vec!["bin", "list", "--json"], |c| c.json),
        (vec!["bin", "list", "--markdown"], |c| c.markdown),
        (vec!["bin", "compact", "--full"], |c| c.full),
        (vec!["bin", "compact", "--verbose"], |c| c.verbose),
        (vec!["bin", "compact", "--session", "abc"], |c| {
            c.session.as_deref() == Some("abc")
        }),
        (vec!["bin", "list", "--harness", "vibe"], |c| {
            c.harness.as_deref() == Some("vibe")
        }),
    ];
    for (args, check) in cases {
        let cli = Cli::try_parse_from(args)
            .unwrap_or_else(|e| panic!("clap rejected flag after subcommand: {e}"));
        assert!(check(&cli));
    }
}
