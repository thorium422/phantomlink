use log::{error, info};

use crate::{cli::opt::SetupArgs, commands::utils, phork};

pub(crate) fn run(setup_args: SetupArgs) -> eyre::Result<()> {
    // check if the user is root
    utils::ensure_user_is_root()?;

    // Check if already set up
    if phork::namespace::is_setup()? {
        error!("Network environment is already set up. Use `teardown` to reset.");
        return Ok(());
    }
    info!("Setting up network environment");
    phork::namespace::setup(setup_args.qdisc_client_config, setup_args.qdisc_server_config)
}
