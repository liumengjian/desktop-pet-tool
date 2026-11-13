; Tauri NSIS 安装钩子（方案 B：安装时优先下载外置 PDF 转换依赖）。
; 失败 / 离线不阻断安装，首次转换时再补下载；仍失败则回退自研链路。

!macro NSIS_HOOK_POSTINSTALL
  ; 文件已就位，拉起主程序带 --fetch-pdf-deps 仅做依赖下载后自行退出。
  ; 退出码 0 = 就绪；非 0 = 失败，记录细节但继续完成安装。
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --fetch-pdf-deps' $0
  ${If} $0 != 0
    DetailPrint "PDF 转换依赖预下载未成功（退出码 $0），将在首次转换时补下载。"
  ${EndIf}
!macroend
