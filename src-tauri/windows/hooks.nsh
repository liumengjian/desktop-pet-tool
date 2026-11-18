; Tauri NSIS 安装钩子（方案 B：安装时优先下载外置 PDF 转换依赖）。
; 失败 / 离线不阻断安装，首次转换时再补下载；仍失败则回退自研链路。
;
; 另外：安装完成即把「文件粉碎」写进当前用户右键菜单（HKCU，免提权），
; 卸载时删掉同名键。键名必须与 src-tauri/src/shred/shell.rs 保持一致：
;   HKCU\Software\Classes\*\shell\dptFileShred
;   HKCU\Software\Classes\Directory\shell\dptFileShred

!macro NSIS_HOOK_POSTINSTALL
  ; 文件已就位，拉起主程序带 --fetch-pdf-deps 仅做依赖下载后自行退出。
  ; 退出码 0 = 就绪；非 0 = 失败，记录细节但继续完成安装。
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --fetch-pdf-deps' $0
  ${If} $0 != 0
    DetailPrint "PDF 转换依赖预下载未成功（退出码 $0），将在首次转换时补下载。"
  ${EndIf}

  ; 右键菜单「文件粉碎」：先写中文菜单文字，程序启动后会按界面语言自动改成对应文案
  WriteRegStr HKCU "Software\Classes\*\shell\dptFileShred" "" "文件粉碎"
  WriteRegStr HKCU "Software\Classes\*\shell\dptFileShred" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr HKCU "Software\Classes\*\shell\dptFileShred" "MultiSelectModel" "Player"
  WriteRegStr HKCU "Software\Classes\*\shell\dptFileShred\command" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --shred $\"%1$\"'

  WriteRegStr HKCU "Software\Classes\Directory\shell\dptFileShred" "" "文件粉碎"
  WriteRegStr HKCU "Software\Classes\Directory\shell\dptFileShred" "Icon" "$INSTDIR\${MAINBINARYNAME}.exe,0"
  WriteRegStr HKCU "Software\Classes\Directory\shell\dptFileShred" "MultiSelectModel" "Player"
  WriteRegStr HKCU "Software\Classes\Directory\shell\dptFileShred\command" "" '$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --shred $\"%1$\"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; 卸载时清掉右键菜单，别在资源管理器里留一条点了没反应的死项
  DeleteRegKey HKCU "Software\Classes\*\shell\dptFileShred\command"
  DeleteRegKey HKCU "Software\Classes\*\shell\dptFileShred"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\dptFileShred\command"
  DeleteRegKey HKCU "Software\Classes\Directory\shell\dptFileShred"
!macroend
