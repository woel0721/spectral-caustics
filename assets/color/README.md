# CIE 官方颜色数据

以下 CSV 和 JSON 均于 2026-10-24 从 CIE 下载，按原始字节保存，没有裁剪、重采样或修改。
两份数据均由 International Commission on Illumination (CIE), Vienna, AT 发布于 2019 年，
许可均为 [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/)，详见各自 metadata 的 `rightsList`。
此许可说明针对这里的数据文件；使用或改编数据时需保留署名与相同方式共享要求。

| 数据 | 官方来源与引用 | 内容 | MD5 |
| --- | --- | --- | --- |
| `CIE_xyz_1931_2deg.csv` | [CIE 1931 2° CMF](https://cie.co.at/datatable/cie-1931-colour-matching-functions-2-degree-observer)，DOI [10.25039/CIE.DS.xvudnb9b](https://doi.org/10.25039/CIE.DS.xvudnb9b) | 360–830 nm，1 nm；列为 λ、x̄、ȳ、z̄；原始来源 CIE 018:2019, Table 6 | `17cca777db64b17170f06f67ce9d3ab7` |
| `CIE_std_illum_D65.csv` | [CIE standard illuminant D65](https://www.cie.co.at/datatable/cie-standard-illuminant-d65)，DOI [10.25039/CIE.DS.hjfjmt59](https://doi.org/10.25039/CIE.DS.hjfjmt59) | 300–830 nm，1 nm；列为 λ、相对 SPD；原始来源 ISO/CIE 11664-2:2022, Table B.1 | `03d4eb9b837c60671627c946fb534deb` |

原始下载链接：

- [CMF CSV](https://files.cie.co.at/Publications-datasets/CIE_xyz_1931_2deg.csv)、[CMF metadata](https://files.cie.co.at/Publications-datasets/CIE_xyz_1931_2deg.csv_metadata.json)。
- [D65 CSV](https://files.cie.co.at/Publications-datasets/CIE_std_illum_D65.csv)、[D65 metadata v2](https://files.cie.co.at/Publications-datasets/CIE_std_illum_D65.csv_metadata_v2.json)。

下载时已核对 metadata 中的 MD5 和 SHA-256。SHA-256 分别为：

- CMF：`fa663e3535a7e0763a745993a1f0a192eb0275ac46ad2d1befd7626841e713c1`。
- D65：`e76f210bffff3d552ef7113025da5f325d5dfec200dd4b878b1a2f3a507032cb`。

`.gitattributes` 保留这些 CSV/JSON 的原始换行字节，避免 Git 自动转换换行后使校验和失效；官方 CRLF 按有效行尾处理。

`src/optics/spectrum.rs` 使用原表，按 metadata 指定的方式进行线性插值，表外返回零。
接口拒绝非正或非有限波长。渲染阶段计划使用 380–780 nm 子区间；截断与有限步长会使积分白点略有误差。
D65 原表以 560 nm 的相对功率为 100；代码按所选波长及积分宽度计算统一的 k，使白光 Y = 1。
这提供相对颜色标尺，不表示真实光源的瓦特数，也没有包含空间光子通量和玻璃透射损耗。
