

use clap::Parser;


// refer https://github.com/clap-rs/clap/tree/master/clap_derive/examples
#[derive(Parser, Debug, Default)]
#[clap(name = "net relay", author, about)]
pub struct Args {
    #[clap(
        long = "leg1",
        long_help = "leg1 url"
    )]
    pub leg1: String,

    #[clap(
        long = "leg2",
        long_help = "leg2 url"
    )]
    pub leg2: String,
}



