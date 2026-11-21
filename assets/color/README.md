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

`cie_xyz` 和 `d65_spd` 查询原表，在相邻节点间线性插值，表外返回零，非正或非有限波长返回 `None`。D65 原表在 560 nm 的相对功率为 100。[CIE 与 D65 查询实现](../../src/spectrum/mod.rs)

源端平行光按配置的波长节点与梯形积分宽度调用 `d65_normalization`，使该组节点积分得到的白光 Y = 1。积分区间和步长会影响所得白点的 X、Z 分量。该归一化只定义相对颜色标尺，空间通量和玻璃透射由传输过程单独计算。[归一化实现](../../src/spectrum/mod.rs) · [光源归一化](../../src/light/mod.rs)

## PBRT RGB 反射基谱

`pbrt_rgb_reflectance.csv` 于 2026-10-28 从 [PBRT v3 的 spectrum.cpp](https://github.com/mmp/pbrt-v3/blob/master/src/core/spectrum.cpp) 提取 `RGB2SpectLambda` 和七个 `RGBRefl2Spect*` 数组：白、青、品红、黄、红、绿、蓝。CSV 只增加列名并按行排列数组，保留全部原始数值字面量，共 32 个 380–720 nm 节点；这些是 RGB 到反射光谱的近似基函数，不是 CIE 标准数据或实测材质光谱。

PBRT 代码和基谱的版权为 Matt Pharr、Greg Humphreys 和 Wenzel Jakob，按 BSD 两条款许可分发；原始版权和完整许可保存在 [PBRT_LICENSE.txt](PBRT_LICENSE.txt)，与上面的 CIE 数据许可分开。

- 方法说明：[PBRT — The SampledSpectrum Class](https://pbr-book.org/3ed-2018/Color_and_Radiometry/The_SampledSpectrum_Class)，以及 Brian Smits, *An RGB-to-Spectrum Conversion for Reflectances* (1999)，[DOI](https://doi.org/10.1080/10867651.1999.10487511)。
- 数据 SHA-256：`bc785beefb8dd372b94d9f9092790fe18a9aeb43064d6defd6639dcafdd6d1c7`。
- `rgb_reflectance` 按 RGB 通道排序组合白、二次色和原色基谱。查询直接在原始节点间线性插值，表外延用最近端点。组合结果乘以 PBRT 的反射率缩放系数 0.94，再限制在 `[0,1]`。RGB 到光谱的映射有多解，D65 下的 RGB 还原可能存在误差。PBRT 的 `SampledSpectrum` 在 400–700 nm 上对波段取平均。[反射谱实现](../../src/spectrum/mod.rs) · [PBRT 波段平均](https://pbr-book.org/3ed-2018/Color_and_Radiometry/The_SampledSpectrum_Class)
- `Material::evaluate_spectral` 接收当前波长并返回标量 `f64` BSDF。Lambertian 在有效反射方向上返回 `rgb_reflectance` 得到的反射率除以 π。光谱相机累积路径贡献时使用 CIE 匹配函数转换为线性 XYZ。[Material 接口](../../src/material/mod.rs) · [Lambertian 实现](../../src/material/lambertian.rs) · [光谱相机积分](../../src/integrator/camera/spectral.rs)

## PBRT RGB 发光基谱与光谱相机

`pbrt_rgb_illuminant.csv` 从 [PBRT v3 官方 spectrum.cpp](https://github.com/mmp/pbrt-v3/blob/master/src/core/spectrum.cpp) 提取 `RGB2SpectLambda` 与七个 `RGBIllum2Spect*` 数组（白、青、品红、黄、红、绿、蓝），32 个 380–720 nm 节点，保留原数值字面量。它与反射基谱是不同的数据；适用同目录的 `PBRT_LICENSE.txt` BSD 两条款许可。提取文件 SHA-256：`def69d81d7fe266ae9575d9d85b3804066108852bbd980446ebe093761c61b24`。

`rgb_emission` 先用 RGB 的最大分量分离 HDR 强度，再按通道排序组合七条发光基谱。组合值的负值截为零，随后除以同一波长的白基谱，乘以归一化 D65 并恢复 HDR 强度。PBRT 的共同 Illuminant 缩放因子在基谱比值中相消。720–780 nm 使用 720 nm 的基谱值。单位 RGB 白色对应归一化 D65，发光谱允许保留 HDR 强度。这是 HDRI、渐变天空和常量背景的光谱近似，色度还原可能存在误差。[发光谱实现](../../src/spectrum/emission.rs)

光谱相机的解析太阳与圆盘照明使用连续归一化的 D65 光谱密度，单位为每 nm。归一化在 380–780 nm 上逐个 1 nm 区间对 SPD 与 ȳ 的乘积做 Simpson 积分。Hero 和 Packet 均在这一波段采样，每条光谱路径的贡献乘 CIE 匹配函数与 `1/(p(λ)·camera_spp)`。源端的离散波长积分宽度不进入相机的采样权重。[连续归一化](../../src/spectrum/emission.rs) · [相机照明](../../src/integrator/camera/transport.rs) · [光谱相机积分](../../src/integrator/camera/spectral.rs)
