# macOS 内置浏览器版本标识验证

验证环境：macOS 26.3、已安装 Safari 26.3，2026-10-16。
在隔离的原生桌面窗口内点击本地测试网页，分别读取 HTTP 请求头与页面呈现的
`navigator.userAgent`。没有调用 Agent 或修改真实文档。

修复前两者均为：

```text
Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)
```

修复后两者均为：

```text
Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.3 Safari/605.1.15
```

实现只在内置网页的 WebKit 配置中，通过公开的 `applicationNameForUserAgent`
补充本机已安装 Safari 的版本；不改工作台标识、不注入页面脚本、不隐藏网站提示，
也不把 WebKit 标成 Chromium。安装信息缺失或格式无效时沿用默认标识。
这修正的是网站识别，不会升级渲染引擎或补充缺失的 Web API。

依据：[WebKit 官方问题 278284](https://bugs.webkit.org/show_bug.cgi?id=278284)、
[Apple 的配置 API](https://developer.apple.com/documentation/webkit/wkwebviewconfiguration/applicationnameforuseragent)。
原生桌面构建、21 项桌面测试和拒绝警告的 Clippy 检查通过。

飞书文档跳转到了登录页，因此此验证**没有确认登录后的版本提示是否消失**，
也没有验证文档的编辑、协作或上传功能。登录后的实际页面需要另行复验。
