fn main() {
    let mut endpoint = turnstone::remote_projection::TurnstoneEndpoint::fixture()
        .expect("Turnstone projection fixture is valid");
    graphshell_stdio::serve_basic(
        &mut endpoint,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
    .expect("Turnstone Graphshell endpoint failed");
}
