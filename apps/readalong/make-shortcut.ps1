$dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$s = (New-Object -ComObject WScript.Shell).CreateShortcut([Environment]::GetFolderPath('Desktop') + '\Readalong.lnk')
$s.TargetPath = Join-Path $dir '.venv\Scripts\pythonw.exe'
$s.Arguments = '"' + (Join-Path $dir 'readalong.py') + '"'
$s.WorkingDirectory = $dir
$s.IconLocation = Join-Path $dir 'readalong.ico'
$s.Description = 'Read PDFs aloud with word highlighting'
$s.Save()
