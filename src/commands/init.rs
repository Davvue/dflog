use crate::cli::InitArgs;

const WORKING_DIR: &str = "working";
const HASH_DIR: &str = ".dflog/hash";
const CMD_OUT_DIR: &str = ".dflog/cmd-out";

pub fn run(init_args: InitArgs) -> std::io::Result<()> {
    std::fs::create_dir_all(init_args.base_path.join(WORKING_DIR))?;
    std::fs::create_dir_all(init_args.base_path.join(HASH_DIR))?;
    std::fs::create_dir_all(init_args.base_path.join(CMD_OUT_DIR))?;

    Ok(())
}