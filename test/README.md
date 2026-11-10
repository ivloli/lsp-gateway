# 测试用例文档

## Gemini测试配置
- **模型名称**: `mock/lsp-gemini`
- **ModelConfig表ID**: `mock/lsp-rewrite`
- **Provider表**: 支持模型名 `rewrite`

## Self集群测试配置
- **模型名称**: `mock/lsp-base` 
- **ModelConfig表ID**: `mock/lsp`

## 测试用例

### 测试用例（一）：Self集群测试

**测试环境**：
- 三个mock server：mock-1、mock-2、mock-3
- mock-1、mock-2为tier0级别
- mock-3为tier-1级别，作为novita

**测试步骤**：
1. mock-1通过超时模拟可用率下降40%
2. 触发自动权重调整机制
3. 优先将流量分流到同级别的mock-2
4. 当mock-2的RPS接近maxrps时
5. 将请求发送到novita（mock-3）
6. 持续测试直到novita报错

### 测试用例（二）：Gemini渠道测试

**测试环境**：
- 三个mock server代表不同渠道：mock-1、mock-2、mock-3
- mock-1、mock-2为tier0级别  
- mock-3为tier-1级别，代表更昂贵的渠道

**测试步骤**：
1. mock-1随机报错模拟可用率下降40%
2. 触发自动权重调整机制
3. 优先将流量分流到同级别的mock-2
4. 由于渠道没有maxrps限制，主要观察mock-2的报错情况
5. 如果mock-2报错，则将请求发送到mock-3
6. 持续测试直到mock-3报错

### 测试用例（三）：self集群reset测试
**测试环境**：
- 三个mock server：mock-1、mock-2
- mock-1、mock-2为tier0级别

**测试步骤**：
1. mock-1随机报错模拟可用率下降40%
2. 触发自动权重调整机制
3. 优先将流量分流到同级别的mock-2
4. mock-2完全扛住，mock-1和mock-2可用率都保持100%
5. 重启mock-1不再随机报错
6. 手动调用reset接口，gateway恢复默认权重
7. 预期不会触发权重调整
