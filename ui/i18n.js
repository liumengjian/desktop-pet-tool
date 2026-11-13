// 界面文案字典：中 / 英。默认中文，语言由配置项 lang（zh | en）决定。
// 用法：I18N.t('docs.start')、I18N.t('docs.fileCount', { n: 3 })；
// 静态文案在 HTML 上用 data-i18n / data-i18n-title / data-i18n-placeholder 标注。
(function () {
  'use strict';

  var DICT = {
    zh: {
      'common.cancel': '取消',
      'common.save': '保存',
      'common.close': '关闭',
      'common.browse': '浏览',
      'common.hide': '收起',

      'input.title': '粘贴文件链接',
      'input.prepare': '准备中…',
      'input.connecting': '连接中…',
      'input.done': '完成',
      'input.needUrl': '请输入文件链接',
      'input.badScheme': '链接需以 http:// 或 https:// 开头',
      'input.downloading': '下载中 {pct}%  ({mb} / {total} MB)',
      'input.downloaded': '已下载 {mb} MB — {name}',
      'input.okCopied': '已复制，直接粘贴',
      'input.okNoClip': '✓ 已下载（剪贴板复制受限）',
      'input.okPathText': '已下载（路径已复制为文本）',

      'docs.title': '文档工具',
      'docs.titleCount': '文档工具 · {n} 个文件',
      'docs.titleCountOne': '文档工具 · 1 个文件',
      'docs.modeConvert': '转换',
      'docs.modeConvertSub': '换成别的格式',
      'docs.modeCompress': '压缩',
      'docs.modeCompressSub': '更小 / 标准 / 更清晰',
      'docs.levelSmall': '更小',
      'docs.levelNormal': '标准',
      'docs.levelHigh': '更清晰',
      'docs.pick': '选择文件',
      'docs.pickMore': '继续添加',
      'docs.start': '开始处理',
      'docs.openDir': '打开临时文件夹',
      'docs.again': '完成',
      'docs.back': '返回',
      'docs.working': '处理中',
      'docs.empty': '把文件拖到悬浮球上，或点下面「选择文件」',
      'docs.remove': '移出列表',
      'docs.hintAddFirst': '先添加文件',
      'docs.hintNoTargetOne': '{name}：当前版本没有可用的目标格式',
      'docs.hintNoTarget': '所选文件在当前版本没有可用的目标格式',
      'docs.hintWillConvert': '将转换为 {fmt}',
      'docs.hintPickTarget': '选一个目标格式',
      'docs.hintNoCompress': '这些文件不支持压缩',
      'docs.hintCompressSkip': '{ok} 个文件可压缩，{skip} 个不支持压缩（会跳过）',
      'docs.hintCompressLevel': '按「{level}」档压缩',
      'docs.hintPickMode': '选一个动作：转换 或 压缩',
      'docs.fileCount': '{n} 个文件',
      'docs.pickFailed': '选择文件失败：{err}',
      'docs.canceling': '正在取消…',
      'docs.doneToast': '已完成 {n} 个文件',
      'docs.successStatus': '处理成功',
      'docs.failedToast': '处理失败',
      'docs.noteCanceled': '任务已取消',

      'kind.image': '图片',
      'kind.word': 'Word',
      'kind.sheet': '表格',
      'kind.slides': '幻灯片',
      'kind.pdf': 'PDF',
      'kind.text': '文本',
      'kind.markdown': 'Markdown',
      'kind.html': '网页',
      'kind.csv': 'CSV',
      'kind.rtf': 'RTF',
      'kind.odf': '开放文档',
      'kind.epub': '电子书',
      'kind.other': '不支持',

      'set.tempPath': '临时文件存放路径',
      'set.tempPathTip': '生成的文件会先保存到这里，复制到剪贴板后可随时清理。',
      'set.alwaysOnTop': '窗口始终置顶',
      'set.closeTitle': '点击悬浮球右上角关闭按钮时',
      'set.closeQuit': '直接退出',
      'set.closeTray': '最小化到系统托盘',
      'set.closeTip': '最小化到托盘后，可从系统托盘图标点击恢复悬浮窗。',
      'set.language': '语言 / Language',
      'set.langTip': '点「保存」后生效，菜单与悬浮球气泡都会跟着换。',
      'set.contact': '如遇到问题，请联系：18725926908（电话&微信）、liumengjianguge@gmail.com（邮箱）',
      'set.pickDirTitle': '选择临时文件存放目录',
    },

    en: {
      'common.cancel': 'Cancel',
      'common.save': 'Save',
      'common.close': 'Close',
      'common.browse': 'Browse',
      'common.hide': 'Hide',

      'input.title': 'Paste a file link',
      'input.prepare': 'Preparing…',
      'input.connecting': 'Connecting…',
      'input.done': 'Done',
      'input.needUrl': 'Enter a file link',
      'input.badScheme': 'Link must start with http:// or https://',
      'input.downloading': 'Downloading {pct}%  ({mb} / {total} MB)',
      'input.downloaded': 'Downloaded {mb} MB — {name}',
      'input.okCopied': 'Copied — paste it now',
      'input.okNoClip': '✓ Downloaded (clipboard blocked)',
      'input.okPathText': 'Downloaded (path copied as text)',

      'docs.title': 'Document tools',
      'docs.titleCount': 'Document tools · {n} files',
      'docs.titleCountOne': 'Document tools · 1 file',
      'docs.modeConvert': 'Convert',
      'docs.modeConvertSub': 'To another format',
      'docs.modeCompress': 'Compress',
      'docs.modeCompressSub': 'Size vs quality',
      'docs.levelSmall': 'Smaller',
      'docs.levelNormal': 'Normal',
      'docs.levelHigh': 'Sharper',
      'docs.pick': 'Choose files',
      'docs.pickMore': 'Add more',
      'docs.start': 'Start',
      'docs.openDir': 'Open temp folder',
      'docs.again': 'Done',
      'docs.back': 'Back',
      'docs.working': 'Working',
      'docs.empty': 'Drag files onto the ball, or click “Choose files” below',
      'docs.remove': 'Remove from list',
      'docs.hintAddFirst': 'Add files first',
      'docs.hintNoTargetOne': '{name}: no target format available in this version',
      'docs.hintNoTarget': 'No target format available for the selected files',
      'docs.hintWillConvert': 'Will convert to {fmt}',
      'docs.hintPickTarget': 'Pick a target format',
      'docs.hintNoCompress': "These files can't be compressed",
      'docs.hintCompressSkip': '{ok} compressible, {skip} skipped',
      'docs.hintCompressLevel': 'Compress at “{level}”',
      'docs.hintPickMode': 'Pick an action: Convert or Compress',
      'docs.fileCount': '{n} files',
      'docs.pickFailed': 'Failed to choose files: {err}',
      'docs.canceling': 'Cancelling…',
      'docs.doneToast': 'Finished {n} files',
      'docs.successStatus': 'Processing succeeded',
      'docs.failedToast': 'Processing failed',
      'docs.noteCanceled': 'Task cancelled',

      'kind.image': 'Image',
      'kind.word': 'Word',
      'kind.sheet': 'Spreadsheet',
      'kind.slides': 'Slides',
      'kind.pdf': 'PDF',
      'kind.text': 'Text',
      'kind.markdown': 'Markdown',
      'kind.html': 'HTML',
      'kind.csv': 'CSV',
      'kind.rtf': 'RTF',
      'kind.odf': 'OpenDocument',
      'kind.epub': 'E-book',
      'kind.other': 'Unsupported',

      'set.tempPath': 'Temp folder',
      'set.tempPathTip': 'Files are saved here first, and can be cleaned up anytime.',
      'set.alwaysOnTop': 'Always on top',
      'set.closeTitle': "When clicking the ball's close button",
      'set.closeQuit': 'Quit',
      'set.closeTray': 'Minimize to tray',
      'set.closeTip': 'Click the tray icon to restore the ball.',
      'set.language': 'Language / 语言',
      'set.langTip': 'Applies after you click Save — menus and ball bubbles switch too.',
      'set.contact': 'Support: 18725926908 (phone & WeChat), liumengjianguge@gmail.com (email)',
      'set.pickDirTitle': 'Choose a temp folder',
    },
  };

  var lang = 'zh';

  function t(key, vars) {
    var table = DICT[lang] || DICT.zh;
    var s = table[key];
    if (s == null) s = DICT.zh[key];
    if (s == null) return key;
    if (vars) {
      Object.keys(vars).forEach(function (k) {
        s = s.split('{' + k + '}').join(String(vars[k]));
      });
    }
    return s;
  }

  function applyStatic() {
    var nodes = document.querySelectorAll('[data-i18n]');
    for (var i = 0; i < nodes.length; i++) {
      nodes[i].textContent = t(nodes[i].getAttribute('data-i18n'));
    }
    nodes = document.querySelectorAll('[data-i18n-title]');
    for (var j = 0; j < nodes.length; j++) {
      nodes[j].setAttribute('title', t(nodes[j].getAttribute('data-i18n-title')));
    }
    nodes = document.querySelectorAll('[data-i18n-placeholder]');
    for (var k = 0; k < nodes.length; k++) {
      nodes[k].setAttribute('placeholder', t(nodes[k].getAttribute('data-i18n-placeholder')));
    }
  }

  window.I18N = {
    get lang() { return lang; },
    setLang: function (next) {
      lang = next === 'en' ? 'en' : 'zh';
      document.documentElement.lang = lang === 'en' ? 'en' : 'zh-CN';
      applyStatic();
    },
    isEn: function () { return lang === 'en'; },
    t: t,
    applyStatic: applyStatic,
  };
})();
