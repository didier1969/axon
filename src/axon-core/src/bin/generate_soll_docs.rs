fn main() -> anyhow::Result<()> { axon_core::soll_docs::generate() }

#[cfg(test)]
mod tests {
    #[test]
    fn entrypoint_links_to_soll_docs() {
        let _: fn() -> anyhow::Result<()> = axon_core::soll_docs::generate;
    }
}
