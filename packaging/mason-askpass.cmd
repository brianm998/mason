@echo off
rem SPDX-License-Identifier: GPL-3.0-or-later
rem
rem SSH_ASKPASS / GIT_ASKPASS helper for masond on Windows: shows a password dialog and prints
rem the answer on stdout, which is where ssh-add and git read it. %1 is the prompt. The
rem passphrase goes dialog -> ssh-add; it never passes through MASON. Cancel exits non-zero.
setlocal
set "MASON_PROMPT=%~1"
if "%MASON_PROMPT%"=="" set "MASON_PROMPT=Passphrase"
powershell -NoProfile -NonInteractive -STA -Command "Add-Type -AssemblyName System.Windows.Forms; $f=New-Object Windows.Forms.Form; $f.Text='MASON'; $f.Width=420; $f.Height=160; $f.StartPosition='CenterScreen'; $l=New-Object Windows.Forms.Label; $l.Text=$env:MASON_PROMPT; $l.Left=10; $l.Top=10; $l.Width=380; $f.Controls.Add($l); $t=New-Object Windows.Forms.TextBox; $t.UseSystemPasswordChar=$true; $t.Left=10; $t.Top=40; $t.Width=380; $f.Controls.Add($t); $ok=New-Object Windows.Forms.Button; $ok.Text='OK'; $ok.Left=220; $ok.Top=75; $ok.DialogResult='OK'; $f.Controls.Add($ok); $c=New-Object Windows.Forms.Button; $c.Text='Cancel'; $c.Left=305; $c.Top=75; $c.DialogResult='Cancel'; $f.Controls.Add($c); $f.AcceptButton=$ok; $f.CancelButton=$c; $f.TopMost=$true; if($f.ShowDialog() -eq 'OK'){[Console]::Out.Write($t.Text)}else{exit 1}"
exit /b %ERRORLEVEL%
