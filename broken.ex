defmodule Malformed do
  def broken_function do
    # missing closing 'end' and syntactically invalid
    if true do
      IO.puts("broken")
  
