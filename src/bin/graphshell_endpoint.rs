fn main() {
    let mut endpoint = merecat::remote_projection::MerecatEndpoint::fixture()
        .expect("Merecat projection fixture is valid");
    graphshell_stdio::serve_basic(
        &mut endpoint,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
    .expect("Merecat Graphshell endpoint failed");
}
