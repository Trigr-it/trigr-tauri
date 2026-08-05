; Synthetic AutoHotkey script covering the hotstring shapes the importer
; must handle. Non-hotstring lines (hotkeys, directives, functions) are
; ignored by the parser.
#Requires AutoHotkey v2.0
#SingleInstance Force

; Plain hotstrings
::btw::by the way
::omw::on my way

; Immediate mode (no ending character required)
:*:sig@::jane.doe@example.com

; Case-sensitive (imported case-insensitive, with a warning)
:C:CEO::Chief Executive Officer

; Raw mode (braces are literal text)
:T:braces::use {curly} braces literally

; Backtick escapes
::twoline::first line`nsecond line

; {Enter} converts to a newline, so this imports
::addr::1 Example Street{Enter}London

; Send-command hotstring (skipped: it drives keys, not text)
::selectall::^a{Del}{Left 3}

; Execute option (skipped: runs code)
:X:calc::Run "calc.exe"

; Hotstring that runs code below (skipped)
::codeblock::
MsgBox "not text"
return

; Multi-line continuation section (imported)
::mysig::
(
Best regards,
Jane Doe
Acme Ltd
)

; Continuation with Join option (skipped: unsupported)
::joined::
( Join|
alpha
beta
)

; Uppercase trigger (lowered on import, with a warning)
::TY::thank you

; A hotkey, not a hotstring: ignored entirely
^j::Send "ctrl j pressed"

/*
Block comment: the parser must not read this as a hotstring
::inside-comment::should not import
*/
