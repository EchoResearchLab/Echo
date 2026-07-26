//! 公司档案页（Leptos）——`GET/PUT/DELETE /api/profiles[/:ticker]` 的 Web 编辑接线。
//!
//! 对标 honeclaw 的 Markdown 长期记忆，但每条数字仍经护栏核对（见 `CompanyProfileDetail`
//! 的 valuation_* 字段），此页只做手动编辑；研究会话自动沉淀 thesis/bull/bear 仍待产品判断
//! （pending 能力），本页不实现。

use crate::api;
use crate::dialog::confirm_destructive;
use crate::format;
use crate::icons::Icon;
use echo_contracts::{
    CompanyProfileDetail, CompanyProfileResponse, CompanyProfileUpsertRequest,
    CompanyProfilesListResponse, Decimal, MutationResponse,
};
use leptos::*;

fn lines_to_vec(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn vec_to_lines(items: &[String]) -> String {
    items.join("\n")
}

#[derive(Clone, Default)]
struct EditForm {
    company_name: String,
    thesis: String,
    research_status: String,
    confidence: String,
    bull: String,
    bear: String,
    monitors: String,
    falsifiers: String,
    profile_md: String,
}

impl From<&CompanyProfileDetail> for EditForm {
    fn from(detail: &CompanyProfileDetail) -> Self {
        Self {
            company_name: detail.company_name.clone().unwrap_or_default(),
            thesis: detail.thesis.clone().unwrap_or_default(),
            research_status: detail.research_status.clone().unwrap_or_default(),
            confidence: detail.confidence.clone().unwrap_or_default(),
            bull: vec_to_lines(&detail.bull),
            bear: vec_to_lines(&detail.bear),
            monitors: vec_to_lines(&detail.monitors),
            falsifiers: vec_to_lines(&detail.falsifiers),
            profile_md: detail.profile_md.clone().unwrap_or_default(),
        }
    }
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn decimal_text(value: Option<Decimal>) -> String {
    value
        .map(|value| value.normalize().to_string())
        .unwrap_or_else(|| "—".to_string())
}

#[component]
pub fn ProfilesSection() -> impl IntoView {
    let (selected, set_selected) = create_signal(None::<String>);
    let (new_ticker, set_new_ticker) = create_signal(String::new());

    let list = create_resource(
        || (),
        |_| api::get::<CompanyProfilesListResponse>("/api/profiles"),
    );

    let detail = create_resource(
        move || selected.get(),
        |ticker| async move {
            match ticker {
                Some(ticker) => Some(
                    api::get::<CompanyProfileResponse>(&format!("/api/profiles/{ticker}")).await,
                ),
                None => None,
            }
        },
    );

    let (form, set_form) = create_signal(EditForm::default());
    let (form_ticker, set_form_ticker) = create_signal(String::new());
    create_effect(move |_| {
        if let Some(Some(Ok(response))) = detail.get() {
            match response.profile {
                Some(profile) => {
                    set_form.set(EditForm::from(&profile));
                    set_form_ticker.set(profile.ticker);
                }
                None => {
                    // 尚无档案（首次为该 ticker 建档）——留一张空表单，ticker 取自选中值。
                    set_form.set(EditForm::default());
                    if let Some(ticker) = selected.get_untracked() {
                        set_form_ticker.set(ticker);
                    }
                }
            }
        }
    });

    let save = create_action(|input: &(String, CompanyProfileUpsertRequest)| {
        let (ticker, body) = input.clone();
        async move {
            api::put::<_, CompanyProfileDetail>(&format!("/api/profiles/{ticker}"), &body)
                .await
                .map(|_| ticker)
        }
    });
    create_effect(move |_| {
        if matches!(save.value().get(), Some(Ok(_))) {
            list.refetch();
            detail.refetch();
        }
    });

    let delete = create_action(|ticker: &String| {
        let ticker = ticker.clone();
        async move {
            api::delete::<MutationResponse>(&format!("/api/profiles/{ticker}"))
                .await
                .map(|_| ticker)
        }
    });
    create_effect(move |_| {
        if let Some(Ok(deleted_ticker)) = delete.value().get() {
            list.refetch();
            if selected.get_untracked().as_deref() == Some(deleted_ticker.as_str()) {
                set_selected.set(None);
            }
        }
    });

    let submit_save = move || {
        let ticker = form_ticker.get().trim().to_uppercase();
        if ticker.is_empty() {
            return;
        }
        let f = form.get();
        save.dispatch((
            ticker,
            CompanyProfileUpsertRequest {
                company_name: non_empty(f.company_name),
                thesis: non_empty(f.thesis),
                research_status: non_empty(f.research_status),
                confidence: non_empty(f.confidence),
                bull: Some(lines_to_vec(&f.bull)),
                bear: Some(lines_to_vec(&f.bear)),
                monitors: Some(lines_to_vec(&f.monitors)),
                falsifiers: Some(lines_to_vec(&f.falsifiers)),
                valuation_method: None,
                valuation_bear: None,
                valuation_base: None,
                valuation_bull: None,
                valuation_current_price: None,
                profile_md: non_empty(f.profile_md),
            },
        ));
    };

    view! {
        <section class="library-section profiles-page">
            <div class="section-heading">
                <div>
                    <p class="section-kicker">"RESEARCH MEMORY"</p>
                    <h2>"公司研究档案"</h2>
                    <p>"把论点、多空逻辑、监控项与证伪条件沉淀为可持续更新的长期记忆。"</p>
                </div>
                <span class="section-status is-neutral">"估值由研究会话写入"</span>
            </div>
            <section class="action-panel profile-create">
                <div class="form-grid watch-form-grid">
                    <label class="form-field">
                        <span>"股票代码"</span>
                        <input
                            placeholder="输入 Ticker，如 NVDA"
                            prop:value=new_ticker
                            on:input=move |ev| set_new_ticker.set(event_target_value(&ev).to_uppercase())
                        />
                    </label>
                    <div class="profile-create-copy">
                        <strong>"新建或打开档案"</strong>
                        <small>"已有档案会直接打开，不会覆盖内容。"</small>
                    </div>
                    <button
                        class="primary-button compact form-submit"
                        disabled=move || new_ticker.get().trim().is_empty()
                        on:click=move |_| {
                            let ticker = new_ticker.get().trim().to_uppercase();
                            if !ticker.is_empty() {
                                set_selected.set(Some(ticker.clone()));
                                set_form_ticker.set(ticker);
                                set_form.set(EditForm::default());
                                set_new_ticker.set(String::new());
                            }
                        }
                    >"打开档案"</button>
                </div>
            </section>

            <div class="profiles-layout">
                <div class="profiles-list">
                    {move || match list.get() {
                        None => view! { <p class="page-state">"读取中…"</p> }.into_view(),
                        Some(Err(error)) => view! { <p class="page-state form-error">{error}</p> }.into_view(),
                        Some(Ok(data)) if data.profiles.is_empty() => {
                            view! { <p class="page-state">"还没有公司档案。"</p> }.into_view()
                        }
                        Some(Ok(data)) => {
                            let active = selected.get();
                            data.profiles.into_iter().map(|item| {
                                let is_active = active.as_deref() == Some(item.ticker.as_str());
                                let go_ticker = item.ticker.clone();
                                view! {
                                    <button
                                        class=if is_active { "session-item-main profile-item is-active" } else { "session-item-main profile-item" }
                                        on:click=move |_| set_selected.set(Some(go_ticker.clone()))
                                    >
                                        <span class="session-item-title">
                                            {item.company_name.clone().unwrap_or_else(|| item.ticker.clone())}
                                            " · " {item.ticker.clone()}
                                        </span>
                                        <span class="session-item-meta">
                                            {item.research_status.clone().unwrap_or_else(|| "未标注".to_string())}
                                            " · 置信 " {item.confidence.clone().unwrap_or_else(|| "—".to_string())}
                                            " · " {item.turn_count} " 轮 · " {format::timestamp(&item.updated_at)}
                                        </span>
                                    </button>
                                }
                            }).collect_view()
                        }
                    }}
                </div>

                <div class="profiles-editor">
                    {move || if selected.get().is_none() {
                        view! { <p class="page-state">"从左侧选择一份档案，或新建一个。"</p> }.into_view()
                    } else {
                        let show_valuation = detail.get()
                            .and_then(|inner| inner)
                            .and_then(Result::ok)
                            .and_then(|response| response.profile);
                        view! {
                            <div class="settings-card profile-form">
                                <div class="profile-editor-head">
                                    <div><p class="section-kicker">"COMPANY DOSSIER"</p><h2>{move || form_ticker.get()}</h2></div>
                                    <span class="section-status"><i></i>"自动保存前需确认"</span>
                                </div>
                                <section class="profile-form-section">
                                    <div class="form-section-title"><strong>"基本判断"</strong><span>"定义当前研究所处阶段"</span></div>
                                    <div class="profile-meta-grid">
                                        <label>"公司名称"
                                            <input
                                                placeholder="公司名称"
                                                prop:value=move || form.get().company_name
                                                on:input=move |ev| set_form.update(|f| f.company_name = event_target_value(&ev))
                                            />
                                        </label>
                                        <label>"研究状态"
                                            <input
                                                placeholder="building / conviction / watch"
                                                prop:value=move || form.get().research_status
                                                on:input=move |ev| set_form.update(|f| f.research_status = event_target_value(&ev))
                                            />
                                        </label>
                                        <label>"置信度"
                                            <input
                                                placeholder="high / medium / low"
                                                prop:value=move || form.get().confidence
                                                on:input=move |ev| set_form.update(|f| f.confidence = event_target_value(&ev))
                                            />
                                        </label>
                                    </div>
                                    <label>"核心论点"
                                        <textarea
                                            rows="3"
                                            placeholder="用一句清晰、可验证的话概括核心判断"
                                            prop:value=move || form.get().thesis
                                            on:input=move |ev| set_form.update(|f| f.thesis = event_target_value(&ev))
                                        ></textarea>
                                    </label>
                                </section>
                                <section class="profile-form-section">
                                    <div class="form-section-title"><strong>"多空框架"</strong><span>"每行一条，保持可扫描"</span></div>
                                    <div class="profile-field-grid">
                                        <label>"多头逻辑"
                                            <textarea
                                                rows="5"
                                                placeholder="增长驱动、优势、上行情景…"
                                                prop:value=move || form.get().bull
                                                on:input=move |ev| set_form.update(|f| f.bull = event_target_value(&ev))
                                            ></textarea>
                                        </label>
                                        <label>"空头逻辑"
                                            <textarea
                                                rows="5"
                                                placeholder="竞争、执行、下行情景…"
                                                prop:value=move || form.get().bear
                                                on:input=move |ev| set_form.update(|f| f.bear = event_target_value(&ev))
                                            ></textarea>
                                        </label>
                                    </div>
                                </section>
                                <section class="profile-form-section">
                                    <div class="form-section-title"><strong>"跟踪与证伪"</strong><span>"把观点转换为可观察的信号"</span></div>
                                    <div class="profile-field-grid">
                                        <label>"监控项"
                                            <textarea
                                                rows="4"
                                                placeholder="需要持续跟踪的指标或事件…"
                                                prop:value=move || form.get().monitors
                                                on:input=move |ev| set_form.update(|f| f.monitors = event_target_value(&ev))
                                            ></textarea>
                                        </label>
                                        <label>"证伪条件"
                                            <textarea
                                                rows="4"
                                                placeholder="什么事实出现时必须推翻当前判断…"
                                                prop:value=move || form.get().falsifiers
                                                on:input=move |ev| set_form.update(|f| f.falsifiers = event_target_value(&ev))
                                            ></textarea>
                                        </label>
                                    </div>
                                </section>
                                <section class="profile-form-section">
                                    <div class="form-section-title"><strong>"研究笔记"</strong><span>"支持 Markdown"</span></div>
                                    <label class="profile-notes">"自由笔记"
                                        <textarea
                                            rows="7"
                                            placeholder="记录上下文、链接和待验证问题…"
                                            prop:value=move || form.get().profile_md
                                            on:input=move |ev| set_form.update(|f| f.profile_md = event_target_value(&ev))
                                        ></textarea>
                                    </label>
                                </section>

                                {show_valuation.map(|p| view! {
                                    <div class="profile-valuation-readout">
                                        <span><small>"估值方法"</small><strong>{p.valuation_method.unwrap_or_else(|| "未核到".to_string())}</strong></span>
                                        <span><small>"熊"</small><strong>{decimal_text(p.valuation_bear)}</strong></span>
                                        <span><small>"基准"</small><strong>{decimal_text(p.valuation_base)}</strong></span>
                                        <span><small>"牛"</small><strong>{decimal_text(p.valuation_bull)}</strong></span>
                                    </div>
                                })}

                                <div class="profile-actions">
                                    <div aria-live="polite">
                                        {move || save.value().get().and_then(Result::err).map(|error| view! { <p class="inline-feedback is-error">{error}</p> })}
                                        {move || matches!(save.value().get(), Some(Ok(_))).then(|| view! { <p class="inline-feedback is-success">"档案已保存"</p> })}
                                    </div>
                                    <button class="primary-button compact" disabled=move || save.pending().get() on:click=move |_| submit_save()>
                                        {move || if save.pending().get() { "保存中…" } else { "保存档案" }}
                                    </button>
                                    <button
                                        class="danger-link"
                                        on:click=move |_| {
                                            let ticker = form_ticker.get_untracked();
                                            if ticker.is_empty() {
                                                return;
                                            }
                                            confirm_destructive(
                                                "删除研究档案",
                                                format!("{ticker} 的论点、多空框架、跟踪与证伪条件将一并删除，无法恢复。"),
                                                "删除档案",
                                                Callback::new(move |_| delete.dispatch(ticker.clone())),
                                            );
                                        }
                                    ><Icon name="trash" />"删除档案"</button>
                                </div>
                            </div>
                        }.into_view()
                    }}
                </div>
            </div>
        </section>
    }
}
