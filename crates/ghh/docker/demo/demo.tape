# VHS script for docs/assets/demo.gif: ghh on a small made-up repository.
# Played by the ghh-demo image (`just demo`); output lands in the working directory, /out, the volume `just demo` mounts.
Output demo.gif

Set Shell bash
Set FontSize 15
Set Width 1280
Set Height 760
Set Padding 16
Set Framerate 20
Set TypingSpeed 40ms
Set Theme "Catppuccin Mocha"

# `enter`: builds ~/shop-api (make-repo), then a `dev` shell.
Hide
Type "exec /opt/demo/enter"
Enter
Sleep 2s
Type "export PS1='\[\e[1;32m\]dev@demo-box\[\e[0m\]:\[\e[1;34m\]~\[\e[0m\]$ '; clear"
Enter
Show

Type "ghh shop-api"
Enter
Sleep 2.5s

# down to a commit inside the checkout pull request; v shows its diff, v again hides it
Down@350ms 5
Sleep 1s
Type "v"
Sleep 3.5s
Type "v"
Sleep 1s

# m: the merge that brought it to main; b: back to where m was pressed; m again for the next step
Type "m"
Sleep 3s
Type "b"
Sleep 2.5s
Type "m"
Sleep 1.5s

# Enter: only what that merge brought in; Backspace: back
Enter
Sleep 3s
Backspace
Sleep 1.5s

# Space: fold the branch away, and back
Space
Sleep 2s
Space
Sleep 1.5s

# search, then filter the list to the matches, then drop the search
Type "/"
Sleep 1s
Type "cart"
Sleep 1s
Enter
Sleep 2.5s
Type "f"
Sleep 3s
Type "F"
Sleep 1.5s

# stashes
Type "S"
Sleep 3s
Down
Sleep 3s
Escape
Sleep 1s

# move the newest commit two rows down: preview, apply, confirm
Type "g"
Sleep 1s
Type "E"
Sleep 1.5s
Down@700ms 2
Sleep 1.5s
Enter
Sleep 2.5s
Type "y"
Sleep 4s

Type "q"
Sleep 1s
