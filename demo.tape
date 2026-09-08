# heft README demo.
# Regenerate:  vhs demo.tape        (needs vhs, ttyd, ffmpeg and a release build)
#
# 1280x740 at this font is 175x32 cells, which is the whole table with no
# column scrolling, so the demo never has to explain a clipped row.

Output "demo.gif"

Set Shell bash
Set FontFamily "Iosevka"
Set FontSize 14
Set LineHeight 1.2
Set Width 1280
Set Height 740
Set Padding 12
Set Framerate 10
Set TypingSpeed 70ms

# heft picks ASCII bars unless a UTF-8 locale is set, and ttyd sets none.
Env LANG "en_US.UTF-8"
# An empty config tree, so a saved view.json on the recording machine cannot
# change what the demo shows.
Env XDG_CONFIG_HOME "/tmp/heft-demo-config"

Hide
Type "export PATH=$PWD/target/release:$PATH" Enter
Type "rm -rf /tmp/heft-demo-config" Enter
Type "clear" Enter
Sleep 1s
Show

Type "heft" Enter
Sleep 4s

# Down to the browser and expand twice: the one row the tree shows is 36
# processes, which is the whole point of the grouping.
Down 4
Sleep 1s
Right
Sleep 2s
Down 1
Right
Sleep 4s
Left
Up 1
Left
Sleep 1s

# Filter keeps matching rows and their parents, so the tree stays a tree.
Type "/"
Sleep 500ms
Type "claude|engined"
Sleep 500ms
Enter
Sleep 4s
Type "/"
Backspace 20
Enter
Sleep 1s

# Sort applies at every level.
Type "c"
Sleep 2s
Type "d"
Sleep 3s

Type "q"
Sleep 1s
