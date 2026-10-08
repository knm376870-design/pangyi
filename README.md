# 旁译 Pangyi

免费的中俄拼音输入法 —— 打字时，候选词旁边多一条俄语译词。

> 灵感来自 [青简 Qingjian](https://qingjian.app)。青简只支持英语 / 日语 / 西班牙语，
> 这个项目专做**中文 → 俄语**，全部数据免费、全部在本机。

## 现状

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| 0 | 数据管线 + 命令行 demo | ✅ 已完成 |
| 1 | 自用原型（悬浮窗 / 输入框） | ⬜ 未开始 |
| 2 | 接入 Windows 输入法框架 | ⬜ 未开始 |
| 3 | 设置界面、学习记录、统计 | ⬜ 未开始 |

## 跑起来

```powershell
python tools/build_dict.py   # 生成 data/zh-ru.tsv
python tools/demo.py         # 交互式；或 python tools/demo.py kaifa diannao
```

## 数据来源

三份都是免费数据，见 [NOTICE.md](NOTICE.md)。

| 数据 | 用途 | 许可 |
| --- | --- | --- |
| CC-CEDICT | 繁/简对照 + 拼音 | CC BY-SA 4.0 |
| FreeDict `zho-rus` | 中文 → 俄语译词 | CC BY-SA 3.0 |
| jieba 词频表 | 候选排序 | MIT |

## 已知限制

- 当前 122,635 条 CC-CEDICT 词条里，**33,384 条**（27.2%）拿到了俄语译词。
  常用词有缺口（如 `xuexi`、`zhongguo` 暂时查不到），需要再接数据源补。
- 还没有词频/整句模型，排序比较朴素。
- 译词带维基百科的重音符（如 `компью́тер`），以后可以按需去掉。

## 许可

代码：MIT（待补 LICENSE）。
数据：CC BY-SA 系列 —— 分发时必须保留署名并以相同方式共享。
