# 颜色与反射基谱数据

## CIE 官方颜色数据

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

## PBRT RGB 反射基谱

`pbrt_rgb_reflectance.csv` 于 2026-10-28 从 [PBRT v3 的 spectrum.cpp](https://github.com/mmp/pbrt-v3/blob/master/src/core/spectrum.cpp) 提取 `RGB2SpectLambda` 和七个 `RGBRefl2Spect*` 数组：白、青、品红、黄、红、绿、蓝。CSV 只增加列名并按行排列数组，保留全部原始数值字面量，共 32 个 380–720 nm 节点；这些是 RGB 到反射光谱的近似基函数，不是 CIE 标准数据或实测材质光谱。

PBRT 代码和基谱的版权为 Matt Pharr、Greg Humphreys 和 Wenzel Jakob，按 BSD 两条款许可分发；原始版权和完整许可保存在 [PBRT_LICENSE.txt](PBRT_LICENSE.txt)，与上面的 CIE 数据许可分开。

- 方法说明：[PBRT — The SampledSpectrum Class](https://pbr-book.org/3ed-2018/Color_and_Radiometry/The_SampledSpectrum_Class)，以及 Brian Smits, *An RGB-to-Spectrum Conversion for Reflectances* (1999)，[DOI](https://doi.org/10.1080/10867651.1999.10487511)。
- 数据 SHA-256：`bc785beefb8dd372b94d9f9092790fe18a9aeb43064d6defd6639dcafdd6d1c7`。
- 实现：`src/optics/spectrum.rs::rgb_reflectance` 按 RGB 通道排序组合白、二次色和原色基谱，沿波长线性插值并在表外延用最近端点。使用 PBRT 的 0.94 反射率缩放；本项目额外将最终反射率上限设为 1（PBRT 原实现仅将负值裁为零），保证各波长 Lambertian 反射能量不超过入射能量。
- 本实现直接查询原始分段线性基谱，没有经过 PBRT 内部 400–700 nm 的分桶平均；RGB→光谱不唯一，D65 下的 RGB 还原不保证精确。CIE 匹配函数继续使用上面的官方表格，表外的 XYZ 响应为零。
