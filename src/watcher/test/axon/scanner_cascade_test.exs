defmodule Axon.ScannerCascadeTest do
  use ExUnit.Case

  test "respects hierarchy and force-includes .md" do
    # On crée un dossier de test avec un .axonignore local
    test_dir = "/tmp/axon_test_cascade"
    File.rm_rf!(test_dir)
    File.mkdir_p!(test_dir)
    
    # 1. Fichiers normaux
    File.write!(Path.join(test_dir, "include_me.txt"), "content")
    
    # 2. Fichier ignoré par le global (.axonignore moteur contient _build)
    File.mkdir_p!(Path.join(test_dir, "_build"))
    File.write!(Path.join(test_dir, "_build/ignored.txt"), "ignored")
    
    # 3. Fichier .md dans un dossier ignoré (Doit être INCLUS par la règle d'or)
    File.write!(Path.join(test_dir, "_build/read_me.md"), "important doc")
    
    # 4. Fichier ignoré localement
    File.write!(Path.join(test_dir, ".axonignore"), "secret.txt\n")
    File.write!(Path.join(test_dir, "secret.txt"), "hidden")
    
    # 5. Fichier forcé localement malgré une règle globale (ex: *.bin est globalement ignoré)
    File.write!(Path.join(test_dir, "important.bin"), "binary")
    # On ajoute la négation dans le .axonignore local
    File.write!(Path.join(test_dir, ".axonignore"), "!important.bin\n", [:append])

    files = Axon.Scanner.scan(test_dir)
    paths = Enum.map(files, fn p -> Path.relative_to(p, test_dir) end)

    # Assertions
    assert "include_me.txt" in paths
    assert "important.bin" in paths
    assert "_build/read_me.md" in paths
    assert not ("secret.txt" in paths)
    assert not ("_build/ignored.txt" in paths)
    
    File.rm_rf!(test_dir)
  end
end
