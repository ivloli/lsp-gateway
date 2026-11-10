//! Error Rate Switching Framework for Supervisor
//!
//! 基于可扩展滑动窗口和双重阈值检测的错误率切流算法
//! 完全按照 error_rate_switching_algorithm.md 文档实现

use crate::monitoring::supervisor::config::SupervisorConfig;
use crate::storage::{ProviderHealthInfo, RedisService};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info};

fn format_series_truncated(series: &[f64], max_items: usize) -> String {
    if series.is_empty() {
        return "[]".to_string();
    }

    if series.len() <= max_items {
        let items: Vec<String> = series.iter().map(|v| format!("{:.4}", v)).collect();
        return format!("[{}]", items.join(", "));
    }

    let show_each_side = max_items / 2;
    let front: Vec<String> = series
        .iter()
        .take(show_each_side)
        .map(|v| format!("{:.4}", v))
        .collect();
    let back: Vec<String> = series
        .iter()
        .skip(series.len() - show_each_side)
        .map(|v| format!("{:.4}", v))
        .collect();

    format!(
        "[{}, ... ({} more) ..., {}]",
        front.join(", "),
        series.len() - max_items,
        back.join(", ")
    )
}

/// 窗口聚合方法
#[derive(Debug, Clone)]
pub enum AggregationMethod {
    Mean,         // 均值 (默认)
    Median,       // 中位数
    P95,          // 95分位数
    WeightedMean, // 时间加权均值
}

/// 滑动窗口配置
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub window_size: Duration,    // 窗口大小 (默认 10s)
    pub slide_interval: Duration, // 滑动间隔 (默认 1s)
    pub history_depth: usize,     // 历史窗口深度 (默认 5)
    pub aggregation_method: AggregationMethod,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            window_size: Duration::from_secs(10),
            slide_interval: Duration::from_secs(1),
            history_depth: 5,
            aggregation_method: AggregationMethod::Mean,
        }
    }
}

/// 趋势检测配置
#[derive(Debug, Clone)]
pub struct DetectionConfig {
    pub rel_rise_threshold: f64, // 相对上升阈值 (默认 0.5 = 50%)
    pub abs_rise_threshold: f64, // 绝对上升阈值 (默认 0.02 = 2%)
    pub rel_fall_threshold: f64, // 相对下降阈值 (默认 0.3 = 30%)
    pub abs_fall_threshold: f64, // 绝对下降阈值 (默认 0.01 = 1%)
    pub epsilon: f64,            // 防除零保护 (默认 0.001)
    pub min_confidence: f64,     // 最小置信度 (默认 0.7)
    pub healthy_threshold: f64,  // 健康状态阈值 (默认 0.01 = 1%)
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            rel_rise_threshold: 0.5,  // 50%
            abs_rise_threshold: 0.02, // 2%
            rel_fall_threshold: 0.3,  // 30%
            abs_fall_threshold: 0.01, // 1%
            epsilon: 0.001,
            min_confidence: 0.7,
            healthy_threshold: 0.01, // 1%
        }
    }
}

/// 趋势检测结果
#[derive(Debug, Clone)]
pub enum TrendResult {
    Deteriorating(f64), // 恶化 + 置信度
    Recovering(f64),    // 恢复 + 置信度
    Stable(f64),        // 稳定 + 置信度
    Healthy(f64),       // 健康（错误率很低）+ 置信度
    Unknown(f64),       // 未知（数据不足）+ 置信度
}

/// 决策结果
#[derive(Debug, Clone)]
pub enum Decision {
    ReduceTraffic(f64),   // 减流 + 减流比例
    IncreaseTraffic(f64), // 加流 + 加流比例
    Maintain,             // 保持现状
}

/// 决策上下文
#[derive(Debug, Clone)]
pub struct DecisionContext {
    pub current_error_rate: f64,
    pub historical_windows: Vec<f64>,
    pub current_traffic_ratio: f64,
    pub consecutive_stable_windows: usize,
    pub time_since_last_action: Duration,
}

/// 趋势检测器接口
pub trait TrendDetector {
    fn detect_trend(&self, windows: &[f64], config: &DetectionConfig) -> TrendResult;
}

/// 双重阈值检测器 (默认实现)
#[derive(Debug)]
pub struct DualThresholdDetector {
    config: DetectionConfig,
}

impl DualThresholdDetector {
    pub fn new(config: DetectionConfig) -> Self {
        Self { config }
    }
}

impl Default for DualThresholdDetector {
    fn default() -> Self {
        Self {
            config: DetectionConfig::default(),
        }
    }
}

impl TrendDetector for DualThresholdDetector {
    fn detect_trend(&self, windows: &[f64], config: &DetectionConfig) -> TrendResult {
        if windows.len() < 2 {
            return TrendResult::Unknown(0.5);
        }

        let current = windows[windows.len() - 1];
        let previous = windows[windows.len() - 2];

        // 计算相对和绝对变化
        let delta_abs = current - previous;
        let delta_rel = if previous > self.config.epsilon {
            delta_abs / previous
        } else if current > 0.0 {
            1.0 // 从0变为非0，相对变化为100%
        } else {
            0.0
        };

        debug!(
            "Trend detection: current={:.4}, previous={:.4}, delta_rel={:.2}%, delta_abs={:.4}",
            current,
            previous,
            delta_rel * 100.0,
            delta_abs
        );

        // 双重阈值检测：恶化条件
        if delta_rel > self.config.rel_rise_threshold && delta_abs > self.config.abs_rise_threshold
        {
            let confidence = self.calculate_confidence(delta_rel, delta_abs, true);
            info!("🔻 Deteriorating trend detected: rel_change={:.1}%, abs_change={:.3}%, confidence={:.2}", 
                  delta_rel * 100.0, delta_abs * 100.0, confidence);
            return TrendResult::Deteriorating(confidence);
        }

        // 双重阈值检测：恢复条件
        if delta_rel < -self.config.rel_fall_threshold
            && delta_abs < -self.config.abs_fall_threshold
        {
            let confidence = self.calculate_confidence(-delta_rel, -delta_abs, false);
            info!("🔺 Recovering trend detected: rel_change={:.1}%, abs_change={:.3}%, confidence={:.2}", 
                  delta_rel * 100.0, delta_abs * 100.0, confidence);
            return TrendResult::Recovering(confidence);
        }

        // 健康状态检测：错误率很低且稳定
        if current <= config.healthy_threshold && previous <= config.healthy_threshold {
            let health_confidence = 1.0 - (current + previous) / (2.0 * config.healthy_threshold);
            info!(
                "💚 Healthy state detected: current={:.3}%, previous={:.3}%, confidence={:.2}",
                current * 100.0,
                previous * 100.0,
                health_confidence
            );
            return TrendResult::Healthy(health_confidence.max(0.7));
        }

        // 稳定状态（中等错误率但无明显趋势）
        let stability_confidence = 1.0 - (delta_rel.abs() + delta_abs.abs() * 10.0).min(1.0);
        debug!(
            "📊 Stable state: current={:.3}%, previous={:.3}%, confidence={:.2}",
            current * 100.0,
            previous * 100.0,
            stability_confidence
        );
        TrendResult::Stable(stability_confidence.max(0.1))
    }
}

impl DualThresholdDetector {
    /// 计算置信度
    fn calculate_confidence(
        &self,
        rel_change: f64,
        abs_change: f64,
        is_deteriorating: bool,
    ) -> f64 {
        let rel_factor = if is_deteriorating {
            (rel_change / self.config.rel_rise_threshold).min(3.0)
        } else {
            (rel_change / self.config.rel_fall_threshold).min(3.0)
        };

        let abs_factor = if is_deteriorating {
            (abs_change / self.config.abs_rise_threshold).min(3.0)
        } else {
            (abs_change / self.config.abs_fall_threshold).min(3.0)
        };

        let confidence = ((rel_factor + abs_factor) / 2.0 * 0.3 + 0.7).min(1.0);
        confidence.max(self.config.min_confidence)
    }
}

/// 决策制定器接口
pub trait DecisionMaker {
    fn make_decision(&self, trend: TrendResult, context: &DecisionContext) -> Decision;
}

/// 阈值决策器 (默认实现)
#[derive(Debug)]
pub struct ThresholdDecisionMaker {
    pub reduce_threshold: f64,          // α - 减流阈值
    pub increase_threshold: f64,        // β - 加流阈值
    pub stable_windows_required: usize, // N - 稳定窗口数
}

impl ThresholdDecisionMaker {
    pub fn conservative() -> Self {
        Self {
            reduce_threshold: 0.05,   // 5%
            increase_threshold: 0.02, // 2%
            stable_windows_required: 3,
        }
    }

    pub fn aggressive() -> Self {
        Self {
            reduce_threshold: 0.03,   // 3%
            increase_threshold: 0.01, // 1%
            stable_windows_required: 2,
        }
    }
}

impl Default for ThresholdDecisionMaker {
    fn default() -> Self {
        Self::conservative()
    }
}

impl DecisionMaker for ThresholdDecisionMaker {
    fn make_decision(&self, trend: TrendResult, context: &DecisionContext) -> Decision {
        match trend {
            TrendResult::Deteriorating(confidence) => {
                // 减流条件：错误率超过阈值 且 检测到恶化趋势
                if context.current_error_rate > self.reduce_threshold {
                    let reduction_ratio =
                        self.calculate_reduction_ratio(context.current_error_rate, confidence);
                    info!("🚨 Decision: Reduce traffic by {:.1}% (error_rate={:.2}%, confidence={:.2})", 
                          reduction_ratio * 100.0, context.current_error_rate * 100.0, confidence);
                    Decision::ReduceTraffic(reduction_ratio)
                } else {
                    Decision::Maintain
                }
            }
            TrendResult::Recovering(_confidence) => {
                // 加流条件：错误率低于恢复阈值 且 连续稳定窗口
                if context.current_error_rate <= self.increase_threshold
                    && context.consecutive_stable_windows >= self.stable_windows_required
                {
                    let increase_ratio = self.calculate_increase_ratio(context.current_error_rate);
                    info!("📈 Decision: Increase traffic by {:.1}% (error_rate={:.2}%, stable_windows={})", 
                          increase_ratio * 100.0, context.current_error_rate * 100.0, context.consecutive_stable_windows);
                    Decision::IncreaseTraffic(increase_ratio)
                } else {
                    Decision::Maintain
                }
            }
            TrendResult::Healthy(_confidence) => {
                // 🆕 健康状态：可以考虑增加流量，但要求更宽松
                if context.consecutive_stable_windows >= 1 {
                    // 健康状态只需要1个稳定窗口
                    let increase_ratio =
                        self.calculate_increase_ratio(context.current_error_rate) * 1.5; // 健康状态可以更积极
                    info!("💚 Decision: Increase traffic by {:.1}% (healthy state, error_rate={:.2}%)", 
                          increase_ratio * 100.0, context.current_error_rate * 100.0);
                    Decision::IncreaseTraffic(increase_ratio.min(0.3)) // 最大30%增加
                } else {
                    Decision::Maintain
                }
            }
            TrendResult::Stable(_confidence) => Decision::Maintain,
            TrendResult::Unknown(_confidence) => {
                // 🆕 未知状态：保守处理，不做调整
                Decision::Maintain
            }
        }
    }
}

impl ThresholdDecisionMaker {
    /// 计算减流比例
    fn calculate_reduction_ratio(&self, error_rate: f64, confidence: f64) -> f64 {
        // 基础减流比例：30-50%
        let base_ratio = 0.3;
        let severity_factor = (error_rate / self.reduce_threshold - 1.0).min(1.0);
        let confidence_factor = confidence;

        (base_ratio + severity_factor * 0.2 + confidence_factor * 0.1).min(0.7)
    }

    /// 计算加流比例
    fn calculate_increase_ratio(&self, error_rate: f64) -> f64 {
        // 基础加流比例：10-20%
        let base_ratio = 0.1;
        let health_factor = (self.increase_threshold - error_rate) / self.increase_threshold;

        (base_ratio + health_factor * 0.1).min(0.2)
    }
}

/// 切流配置
pub struct SwitchingConfig {
    pub window: WindowConfig,
    pub detector: Box<dyn TrendDetector + Send + Sync>,
    pub decision_maker: Box<dyn DecisionMaker + Send + Sync>,
}

impl SwitchingConfig {
    /// 保守配置 - 适合生产环境
    pub fn conservative() -> Self {
        Self {
            window: WindowConfig::default(),
            detector: Box::new(DualThresholdDetector::default()),
            decision_maker: Box::new(ThresholdDecisionMaker::conservative()),
        }
    }

    /// 敏感配置 - 适合测试环境
    pub fn sensitive() -> Self {
        Self {
            window: WindowConfig {
                window_size: Duration::from_secs(5),
                slide_interval: Duration::from_millis(500),
                history_depth: 10,
                aggregation_method: AggregationMethod::P95,
            },
            detector: Box::new(DualThresholdDetector::default()),
            decision_maker: Box::new(ThresholdDecisionMaker::aggressive()),
        }
    }
}

/// Supervisor 层的错误率切流器
/// 基于可扩展滑动窗口框架的完整实现
pub struct ErrorRateSwitcher {
    redis_service: Arc<RedisService>,

    /// 每个 provider 的连续稳定窗口计数
    stable_window_counts: HashMap<String, usize>,
    /// 每个 provider 的上次动作时间
    last_action_times: HashMap<String, Instant>,
    /// 切流配置
    switching_config: SwitchingConfig,
}

impl ErrorRateSwitcher {
    /// 创建新的错误率切流器
    pub fn new(redis_service: Arc<RedisService>, switching_config: SwitchingConfig) -> Self {
        Self {
            redis_service,
            stable_window_counts: HashMap::new(),
            last_action_times: HashMap::new(),
            switching_config,
        }
    }

    /// 从 SupervisorConfig 创建（向后兼容）
    pub fn from_supervisor_config(
        redis_service: Arc<RedisService>,
        _config: &SupervisorConfig,
    ) -> Self {
        let switching_config = SwitchingConfig::conservative();

        let switcher = Self::new(redis_service, switching_config);
        switcher
    }

    /// 处理所有 provider 的错误率切流逻辑
    /// 使用有序的时间序列数据进行趋势分析
    pub async fn process_error_rate_switching(
        &mut self,
        health_infos: &mut Vec<ProviderHealthInfo>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        debug!(
            "Processing error rate switching for {} providers",
            health_infos.len()
        );

        // 🔍 一次性获取所有 provider 的指标数据 (按 provider_id 分组)
        let all_provider_metrics = match self.redis_service.get_all_provider_metrics().await {
            Ok(metrics_map) => metrics_map,
            Err(e) => {
                debug!("Failed to get all provider metrics: {}", e);
                return Ok(());
            }
        };

        let mut adjustments_made = 0;

        for health_info in health_infos.iter_mut() {
            // 获取当前 provider 的所有指标数据
            let provider_metrics = match all_provider_metrics.get(&health_info.provider_id) {
                Some(metrics) => metrics,
                None => {
                    debug!("No metrics found for provider: {}", health_info.provider_id);
                    continue;
                }
            };

            if provider_metrics.is_empty() {
                debug!(
                    "No metrics data available for provider: {}",
                    health_info.provider_id
                );
                continue;
            }

            // 🔢 将指标转换为有序的错误率时间序列 (按时间戳排序)
            let mut error_rate_data: Vec<(u64, f64)> = provider_metrics
                .iter()
                .map(|m| {
                    let error_rate = if m.success_count + m.fail_count > 0 {
                        m.fail_count as f64 / (m.success_count + m.fail_count) as f64
                    } else {
                        0.0
                    };
                    (m.timestamp, error_rate)
                })
                .collect();

            // 按时间戳排序 (旧→新)
            error_rate_data.sort_by_key(|&(timestamp, _)| timestamp);

            // 提取错误率序列
            let error_rate_series: Vec<f64> =
                error_rate_data.iter().map(|&(_, rate)| rate).collect();

            if error_rate_series.len() < 2 {
                debug!(
                    "Insufficient time series data for {}: {} points (need >= 2)",
                    health_info.provider_id,
                    error_rate_series.len()
                );
                continue;
            }

            let current_error_rate = error_rate_series.last().copied().unwrap_or(0.0);

            debug!(
                "Provider {} time series: {:?} (current: {:.4})",
                health_info.provider_id, error_rate_series, current_error_rate
            );

            // 🔍 使用有序时间序列进行趋势检测和决策
            if let Some((decision, trend_str)) = self
                .make_switching_decision_with_time_series(
                    &health_info.provider_id,
                    &error_rate_series,
                )
                .await
            {
                // 设置趋势信息
                health_info.error_rate_trend = Some(trend_str);

                let adjustment_factor = match decision {
                    Decision::ReduceTraffic(ratio) => 1.0 - ratio,
                    Decision::IncreaseTraffic(ratio) => 1.0 + ratio,
                    Decision::Maintain => continue,
                };

                // 应用调整到 health_score
                let original_score = health_info.health_score;
                health_info.health_score = (health_info.health_score * adjustment_factor)
                    .max(0.01)
                    .min(1.0);

                info!("🔄 Error rate switching for {}: series={}, health_score: {:.3} -> {:.3} (factor: {:.3})",
                      health_info.provider_id, format_series_truncated(&error_rate_series, 6), original_score, health_info.health_score, adjustment_factor);

                adjustments_made += 1;

                // 记录动作时间
                self.last_action_times
                    .insert(health_info.provider_id.clone(), Instant::now());
            }
        }

        if adjustments_made > 0 {
            info!(
                "Applied error rate switching to {}/{} providers",
                adjustments_made,
                health_infos.len()
            );
        }

        Ok(())
    }

    /// 基于有序时间序列数据进行切流决策
    async fn make_switching_decision_with_time_series(
        &mut self,
        provider_id: &str,
        error_rate_series: &[f64],
    ) -> Option<(Decision, String)> {
        if error_rate_series.len() < 2 {
            debug!(
                "Insufficient time series data for {}: {} points (need >= 2)",
                provider_id,
                error_rate_series.len()
            );
            return None;
        }

        let current_error_rate = error_rate_series.last().copied().unwrap_or(0.0);

        debug!(
            "Making switching decision for {}: current_rate={:.4}, time_series={:?}",
            provider_id, current_error_rate, error_rate_series
        );

        // 趋势检测 - 直接使用有序时间序列数据
        let detection_config = DetectionConfig::default();
        let trend = self
            .switching_config
            .detector
            .detect_trend(error_rate_series, &detection_config);

        debug!(
            "Trend detection result for {}: {:?}",
            provider_id,
            match &trend {
                TrendResult::Deteriorating(conf) =>
                    format!("Deteriorating(confidence={:.2})", conf),
                TrendResult::Recovering(conf) => format!("Recovering(confidence={:.2})", conf),
                TrendResult::Stable(conf) => format!("Stable(confidence={:.2})", conf),
                TrendResult::Healthy(conf) => format!("Healthy(confidence={:.2})", conf),
                TrendResult::Unknown(conf) => format!("Unknown(confidence={:.2})", conf),
            }
        );

        // 更新稳定窗口计数
        let stable_count = match trend {
            TrendResult::Stable(_) | TrendResult::Healthy(_) => {
                let count = self.stable_window_counts.get(provider_id).unwrap_or(&0) + 1;
                self.stable_window_counts
                    .insert(provider_id.to_string(), count);
                count
            }
            _ => {
                self.stable_window_counts.insert(provider_id.to_string(), 0);
                0
            }
        };

        // 构建决策上下文
        let time_since_last_action = self
            .last_action_times
            .get(provider_id)
            .map(|t| Instant::now().duration_since(*t))
            .unwrap_or(Duration::from_secs(3600)); // 默认1小时

        let context = DecisionContext {
            current_error_rate,
            historical_windows: error_rate_series.to_vec(),
            current_traffic_ratio: 1.0, // 简化实现
            consecutive_stable_windows: stable_count,
            time_since_last_action,
        };

        debug!(
            "Decision context for {}: error_rate={:.4}, stable_windows={}, time_since_action={}s",
            provider_id,
            current_error_rate,
            stable_count,
            time_since_last_action.as_secs()
        );

        // 🎯 制定决策 - 使用时间序列趋势！
        let decision = self
            .switching_config
            .decision_maker
            .make_decision(trend.clone(), &context);

        let trend_str = match &trend {
            TrendResult::Deteriorating(_) => "Deteriorating",
            TrendResult::Recovering(_) => "Recovering",
            TrendResult::Stable(_) => "Stable",
            TrendResult::Healthy(_) => "Healthy",
            TrendResult::Unknown(_) => "Unknown",
        }
        .to_string();

        debug!(
            "Decision result for {}: {:?}",
            provider_id,
            match &decision {
                Decision::ReduceTraffic(ratio) => format!("ReduceTraffic({:.1}%)", ratio * 100.0),
                Decision::IncreaseTraffic(ratio) =>
                    format!("IncreaseTraffic({:.1}%)", ratio * 100.0),
                Decision::Maintain => "Maintain".to_string(),
            }
        );

        // 总是返回决策和趋势信息
        Some((decision, trend_str))
    }
}
