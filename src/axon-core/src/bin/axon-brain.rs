fn main() -> anyhow::Result<()> {
    axon_core::runtime_boot::run_brain()
}

#[cfg(test)]
mod tests {
    #[test]
    fn entrypoint_links_to_runtime_boot() {
        let _: fn() -> anyhow::Result<()> = axon_core::runtime_boot::run_brain;
    }
}
