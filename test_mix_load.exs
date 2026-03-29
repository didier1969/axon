Mix.start()
Code.require_file("/tmp/dummy_umbrella/apps/child_a/mix.exs")
config = Mix.Project.config()
IO.inspect(config)
