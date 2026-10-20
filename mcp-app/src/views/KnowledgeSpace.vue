<template>
  <div
    :class="[
      'overflow-hidden bg-cm-canvas font-cm-sans text-cm-primary',
      embedded ? 'h-full min-h-full' : 'min-h-screen'
    ]"
  >
    <header v-if="!embedded" class="flex h-[60px] items-center gap-7 border-b border-cm-divider bg-cm-panel px-[22px] max-[980px]:gap-3">
      <div class="flex min-w-[300px] items-center gap-3 text-xl max-[980px]:min-w-0">
        <img class="h-8 w-8 rounded-md object-contain" :src="systemInfo.appLogo" alt="" />
        <strong class="max-[980px]:hidden">{{ systemInfo.appName }}</strong>
        <span class="rounded-[7px] border border-cm-default bg-cm-accent-soft px-2 py-0.5 text-xs text-cm-accent max-[980px]:hidden">
          {{ systemInfo.appVersion }}
        </span>
      </div>

      <nav class="flex flex-1 items-center justify-center gap-6">
        <button class="cursor-pointer rounded-lg border border-cm-default bg-cm-input px-[18px] py-2 text-base text-cm-secondary transition hover:border-cm-strong hover:bg-cm-accent-soft hover:text-cm-primary" @click="router.push('/')">任务视图</button>
        <button class="cursor-pointer rounded-lg border border-cm-default bg-cm-input px-[18px] py-2 text-base text-cm-secondary transition hover:border-cm-strong hover:bg-cm-accent-soft hover:text-cm-primary" @click="router.push('/task-center')">流程管理</button>
        <button class="cursor-pointer rounded-lg border border-cm-strong bg-cm-accent-soft px-[18px] py-2 text-base text-cm-primary shadow-cm-glow-soft">知识空间</button>
      </nav>

      <div class="flex items-center gap-3">
        <button class="h-[34px] w-[34px] cursor-pointer rounded-lg border border-cm-default bg-cm-input text-cm-secondary transition hover:border-cm-strong hover:bg-cm-accent-soft hover:text-cm-primary" title="刷新" @click="refreshAll">⟳</button>
        <span class="inline-flex h-[34px] w-[34px] items-center justify-center rounded-full bg-[var(--cm-brand-gradient-diagonal)] font-bold text-cm-inverse">{{ userInitial }}</span>
        <span class="text-cm-secondary max-[980px]:hidden">{{ currentUser?.username || '用户' }}</span>
      </div>
    </header>

    <main
      :class="[
        'grid min-h-0 bg-cm-canvas max-[980px]:grid-cols-1',
        embedded ? 'h-full grid-cols-[360px_minmax(0,1fr)]' : 'h-[calc(100vh-60px)] grid-cols-[360px_minmax(0,1fr)]'
      ]"
    >
      <aside class="min-h-0 overflow-auto border-r border-cm-divider bg-cm-sidebar px-4 py-[18px] max-[980px]:h-[280px] max-[980px]:border-b max-[980px]:border-r-0 max-[980px]:border-cm-divider">
        <section class="mb-4 flex items-center justify-between">
          <h2 class="m-0 text-lg">知识空间</h2>
          <button :class="getPanelButtonClasses(true, 'px-3.5 py-1.5')" @click="openCreateDialog">+ 新建</button>
        </section>

        <label class="flex h-10 items-center gap-2 rounded-[9px] border border-cm-default bg-cm-input px-3 text-cm-tertiary">
          <span>⌕</span>
          <input v-model="kbFilter" class="w-full border-0 bg-transparent font-inherit text-cm-primary outline-none placeholder:text-cm-tertiary" placeholder="搜索知识库" />
        </label>

        <section v-if="channelKnowledgeBases.length" class="mt-6">
          <h3 class="mb-2.5 mt-0 text-[15px] text-cm-secondary">频道知识库</h3>
          <button
            v-for="kb in channelKnowledgeBases"
            :key="kb.id"
            :class="getKnowledgeItemClasses(kb)"
            @click="selectKnowledgeBase(kb)"
          >
            <span class="text-cm-accent">▣</span>
            <span class="min-w-0">
              <span class="block overflow-hidden text-ellipsis whitespace-nowrap font-bold">{{ channelKbName(kb.name) }}</span>
              <span class="block overflow-hidden text-ellipsis whitespace-nowrap text-cm-tertiary">频道 · {{ kb.file_count || 0 }} 文档</span>
            </span>
            <span class="inline-flex items-center gap-1.5 opacity-70 group-hover:opacity-100">
              <span :class="getKnowledgeActionClasses()" title="编辑" @click.stop="openEditDialog(kb)">
                <svg class="h-3.5 w-3.5 fill-none stroke-current stroke-2 [stroke-linecap:round] [stroke-linejoin:round]" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M14.7 6.3a4 4 0 0 0-5 5L4 17v3h3l5.7-5.7a4 4 0 0 0 5-5l-2.3 2.3-3-3z" />
                </svg>
              </span>
              <span :class="getKnowledgeActionClasses(true)" title="删除" @click.stop="removeKnowledgeBase(kb)">
                <svg class="h-3.5 w-3.5 fill-none stroke-current stroke-2 [stroke-linecap:round] [stroke-linejoin:round]" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M4 7h16" />
                  <path d="M9 7V5h6v2" />
                  <path d="M6 7l1 13h10l1-13" />
                  <path d="M10 11v5" />
                  <path d="M14 11v5" />
                </svg>
              </span>
            </span>
          </button>
        </section>

        <section class="mt-6">
          <h3 class="mb-2.5 mt-0 text-[15px] text-cm-secondary">公共知识空间</h3>
          <button
            v-for="kb in publicKnowledgeBases"
            :key="kb.id"
            :class="getKnowledgeItemClasses(kb)"
            @click="selectKnowledgeBase(kb)"
          >
            <span class="text-cm-accent">▣</span>
            <span class="min-w-0">
              <span class="block overflow-hidden text-ellipsis whitespace-nowrap font-bold">{{ kb.name }}</span>
              <span class="block overflow-hidden text-ellipsis whitespace-nowrap text-cm-tertiary">全平台公开 · {{ kb.file_count || 0 }} 文档</span>
            </span>
            <span class="inline-flex items-center gap-1.5 opacity-70 group-hover:opacity-100">
              <span :class="getKnowledgeActionClasses()" title="编辑" @click.stop="openEditDialog(kb)">
                <svg class="h-3.5 w-3.5 fill-none stroke-current stroke-2 [stroke-linecap:round] [stroke-linejoin:round]" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M14.7 6.3a4 4 0 0 0-5 5L4 17v3h3l5.7-5.7a4 4 0 0 0 5-5l-2.3 2.3-3-3z" />
                </svg>
              </span>
              <span :class="getKnowledgeActionClasses(true)" title="删除" @click.stop="removeKnowledgeBase(kb)">
                <svg class="h-3.5 w-3.5 fill-none stroke-current stroke-2 [stroke-linecap:round] [stroke-linejoin:round]" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M4 7h16" />
                  <path d="M9 7V5h6v2" />
                  <path d="M6 7l1 13h10l1-13" />
                  <path d="M10 11v5" />
                  <path d="M14 11v5" />
                </svg>
              </span>
            </span>
          </button>
        </section>

        <section class="mt-6">
          <h3 class="mb-2.5 mt-0 text-[15px] text-cm-secondary">个人知识空间</h3>
          <button
            v-for="kb in privateKnowledgeBases"
            :key="kb.id"
            :class="getKnowledgeItemClasses(kb)"
            @click="selectKnowledgeBase(kb)"
          >
            <span class="text-cm-info">▤</span>
            <span class="min-w-0">
              <span class="block overflow-hidden text-ellipsis whitespace-nowrap font-bold">{{ kb.name }}</span>
              <span class="block overflow-hidden text-ellipsis whitespace-nowrap text-cm-tertiary">仅自己可见 · {{ kb.file_count || 0 }} 文档</span>
            </span>
            <span class="inline-flex items-center gap-1.5 opacity-70 group-hover:opacity-100">
              <span :class="getKnowledgeActionClasses()" title="编辑" @click.stop="openEditDialog(kb)">
                <svg class="h-3.5 w-3.5 fill-none stroke-current stroke-2 [stroke-linecap:round] [stroke-linejoin:round]" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M14.7 6.3a4 4 0 0 0-5 5L4 17v3h3l5.7-5.7a4 4 0 0 0 5-5l-2.3 2.3-3-3z" />
                </svg>
              </span>
              <span :class="getKnowledgeActionClasses(true)" title="删除" @click.stop="removeKnowledgeBase(kb)">
                <svg class="h-3.5 w-3.5 fill-none stroke-current stroke-2 [stroke-linecap:round] [stroke-linejoin:round]" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M4 7h16" />
                  <path d="M9 7V5h6v2" />
                  <path d="M6 7l1 13h10l1-13" />
                  <path d="M10 11v5" />
                  <path d="M14 11v5" />
                </svg>
              </span>
            </span>
          </button>
          <p v-if="!privateKnowledgeBases.length" class="text-cm-tertiary">暂无个人知识库</p>
        </section>

        <section v-if="kbTotal > KB_PAGE_SIZE" class="mt-6 flex items-center justify-between gap-2 border-t border-cm-divider pt-4 text-sm text-cm-secondary">
          <button :class="getPanelButtonClasses(false, 'px-3 py-1.5')" :disabled="kbPage <= 1 || listLoading" @click="changeKbPage(kbPage - 1)">上一页</button>
          <span class="text-cm-tertiary">{{ kbPage }} / {{ kbTotalPages }}</span>
          <button :class="getPanelButtonClasses(false, 'px-3 py-1.5')" :disabled="kbPage >= kbTotalPages || listLoading" @click="changeKbPage(kbPage + 1)">下一页</button>
        </section>
      </aside>

      <section
        :class="[
          'min-h-0 overflow-auto bg-cm-canvas px-7 pb-10 pt-[22px] text-cm-primary max-[980px]:p-4',
          embedded ? 'max-[980px]:h-[calc(100vh-396px)]' : 'max-[980px]:h-[calc(100vh-340px)]'
        ]"
      >
        <section class="flex justify-between gap-5 border-b border-cm-divider bg-transparent px-5 py-[18px] max-[980px]:flex-col">
          <div>
            <div class="flex items-center gap-2.5">
              <span class="text-[22px] text-cm-accent">▣</span>
              <h1 class="m-0 text-xl">{{ channelKbName(selectedDetail?.name || selectedKb?.name) || '知识库' }}</h1>
              <span :class="getVisibilityPillClasses()">{{ isSelectedPublic ? '可检索' : '私有' }}</span>
              <span class="rounded-[7px] border border-cm-default bg-cm-surface-muted px-2 py-0.5 text-xs text-cm-tertiary">{{ isSelectedPublic ? '全平台公开' : '仅自己可见' }}</span>
            </div>
            <p class="ml-8 mt-1.5 text-cm-tertiary">{{ selectedDetail?.description || selectedKb?.description || '暂无描述' }}</p>
          </div>
          <div class="flex items-center gap-2.5">
            <button :class="getPanelButtonClasses(false, 'h-[38px] px-3.5')" @click="reparseSelected" :disabled="!selectedKbId || actionLoading">重新解析</button>
            <!-- <button :class="getPanelButtonClasses(false, 'h-[38px] px-3.5')" @click="openWiki" :disabled="!selectedKbId">Wiki</button> -->
            <!-- <button :class="getPanelButtonClasses(false, 'h-[38px] px-3.5')" @click="openEditDialog" :disabled="!selectedKbId">设置</button> -->
          </div>
        </section>

        <section v-if="isSelectedChannelKb" class="mt-[18px] rounded-[10px] border border-cm-default bg-cm-surface px-5 pb-5 pt-[18px]" aria-labelledby="channel-overview-title">
          <div class="mb-4 flex items-center justify-between gap-5 max-[980px]:items-start">
            <div>
              <div class="flex items-center gap-2 text-cm-accent [&_.el-icon]:text-[19px]">
                <el-icon><DataAnalysis /></el-icon>
                <h2 id="channel-overview-title" class="m-0 text-base text-cm-heading">频道知识概览</h2>
              </div>
              <p class="ml-[27px] mt-1 text-xs text-cm-tertiary">
                资料类型与处理状态
                <span v-if="!channelOverviewLoading">
                  · 已就绪 {{ statusBreakdown.completed || 0 }} · 处理中 {{ statusBreakdown.processing || 0 }} · 失败 {{ statusBreakdown.failed || 0 }}
                </span>
              </p>
            </div>
            <span :class="getSyncPillClasses()">
              <span class="h-1.5 w-1.5 rounded-full bg-current shadow-[0_0_8px_currentColor]"></span>
              {{ statusBreakdown.processing > 0 ? '同步处理中' : '自动同步' }}
            </span>
          </div>

          <div class="grid grid-cols-6 gap-2.5 max-[980px]:grid-cols-2 min-[981px]:max-[1440px]:grid-cols-3" :aria-busy="channelOverviewLoading">
            <button
              v-for="card in channelOverviewCards"
              :key="card.key"
              type="button"
              :class="getOverviewCardClasses(card)"
              :aria-pressed="activeChannelTag === card.tag"
              @click="selectChannelTag(card.tag)"
            >
              <span :class="getOverviewIconClasses(card)">
                <el-icon><component :is="card.icon" /></el-icon>
              </span>
              <span class="min-w-0 flex-1">
                <span class="block overflow-hidden text-ellipsis whitespace-nowrap text-[13px] font-bold text-cm-heading">{{ card.label }}</span>
                <span class="mt-1 block overflow-hidden text-ellipsis whitespace-nowrap text-[11px] text-cm-tertiary">{{ card.description }}</span>
              </span>
              <strong class="flex-none text-xl text-cm-accent [font-variant-numeric:tabular-nums]">{{ channelOverviewLoading ? '—' : card.count }}</strong>
            </button>
          </div>
        </section>

        <section
          :class="[
            'mt-6 flex min-h-[164px] flex-col items-center justify-center gap-2 rounded-[10px] border border-dashed border-cm-default bg-cm-surface',
            isDragging ? 'border-cm-focus bg-cm-accent-soft' : ''
          ]"
          @dragenter.prevent="isDragging = true"
          @dragover.prevent="isDragging = true"
          @dragleave.prevent="isDragging = false"
          @drop.prevent="handleDrop"
        >
          <input ref="fileInputRef" type="file" multiple class="hidden" @change="handleFilePicked" />
          <div class="text-[44px] leading-none text-cm-accent">⇧</div>
          <strong class="text-cm-heading">拖拽文件到此处，或<button class="mx-1 cursor-pointer border-0 bg-transparent font-inherit font-extrabold text-cm-accent" @click="fileInputRef?.click()">点击选择批量上传</button></strong>
          <span class="text-cm-tertiary">支持 PDF / Word / Excel / Markdown / TXT，单文件建议不超过 50MB</span>
        </section>

        <section class="mt-[22px] rounded-[10px] border border-cm-default bg-cm-surface p-5">
          <label class="mb-3.5 flex gap-2 text-cm-accent">
            <span>⌕</span>
            <strong>检索</strong>
          </label>
          <div class="flex gap-3">
            <input v-model="searchQuery" class="h-[42px] w-full rounded-[9px] border border-cm-default bg-cm-input px-3.5 text-cm-primary outline-none placeholder:text-cm-tertiary focus:border-cm-focus" placeholder="输入查询语句，验证检索是否准确" @keyup.enter="runSearch" />
            <button :class="getPanelButtonClasses(true, 'w-[92px]')" :disabled="!searchQuery.trim() || searchLoading" @click="runSearch">检索</button>
          </div>
          <div v-if="searchResults.length" class="mt-4 grid gap-2.5">
            <div class="flex items-center justify-between text-[13px] text-cm-secondary">
              <span>检索结果 {{ searchResults.length }} 条</span>
              <button type="button" class="h-7 w-7 cursor-pointer rounded-md border border-cm-default bg-cm-surface-muted text-cm-secondary transition hover:border-cm-focus hover:text-cm-heading" title="关闭检索结果" @click="clearSearchResults">×</button>
            </div>
            <article v-for="item in searchResults" :key="item.id" class="rounded-lg border border-cm-default bg-cm-surface-muted px-3.5 py-3">
              <div class="flex items-center justify-between gap-3.5">
                <strong>{{ item.file?.filename || `切片 #${item.id}` }}</strong>
                <span class="text-xs text-cm-accent">{{ formatScore(item.score) }}</span>
              </div>
              <p class="my-2 overflow-hidden text-cm-primary [display:-webkit-box] [-webkit-box-orient:vertical] [-webkit-line-clamp:3]">{{ item.content }}</p>
              <small class="text-cm-tertiary">{{ item.kb?.name || selectedDetail?.name || '知识库' }}</small>
            </article>
          </div>
        </section>

        <section class="mt-[22px] rounded-[10px] border border-cm-default bg-cm-surface p-5">
          <div class="mb-3 flex items-center justify-between gap-[18px]">
            <h2 class="m-0 text-lg text-cm-heading">
              文档列表<span v-if="activeChannelTagLabel"> · {{ activeChannelTagLabel }}</span> ({{ documentTotal }})
            </h2>
            <div class="flex items-center gap-3 text-[13px] text-cm-tertiary">
              <span>已完成 {{ statusBreakdown.completed || 0 }}</span>
              <span>处理中 {{ statusBreakdown.processing || 0 }}</span>
              <span>失败 {{ statusBreakdown.failed || 0 }}</span>
            </div>
          </div>

          <div v-if="detailLoading" class="p-9 text-center text-cm-tertiary">加载知识库中...</div>
          <div v-else-if="!documentTotal" class="p-9 text-center text-cm-tertiary">暂无文档</div>
          <div v-else class="border-t border-cm-divider">
            <div
              v-for="file in files"
              :key="file.id"
              class="grid min-h-[52px] grid-cols-[24px_minmax(0,1fr)_86px_96px_32px_32px_32px_32px] items-center gap-2.5 border-b border-cm-divider max-[980px]:grid-cols-[24px_minmax(0,1fr)_68px_74px] max-[980px]:[&>button]:hidden"
            >
              <span class="text-cm-secondary">▤</span>
              <span class="min-w-0 overflow-hidden text-ellipsis whitespace-nowrap" :title="file.filename">{{ cleanFilename(file.filename) }}</span>
              <span class="text-cm-tertiary">{{ formatSize(file.size) }}</span>
              <span
                :class="getDocumentStatusClasses(file.status)"
              >
                <el-icon class="leading-none"><component :is="statusMeta(file.status).icon" /></el-icon>
                {{ statusMeta(file.status).text }}
                <span
                  v-if="isFailedStatus(file.status)"
                  class="absolute bottom-[calc(100%+8px)] right-0 z-20 hidden max-h-[260px] w-[min(360px,70vw)] cursor-default whitespace-normal rounded-lg border border-[color-mix(in_srgb,var(--cm-status-danger)_32%,transparent)] bg-cm-panel px-3 py-2.5 text-cm-primary no-underline shadow-[0_14px_32px_color-mix(in_srgb,var(--cm-bg-canvas)_42%,transparent)] group-hover:block"
                >
                  <span class="mb-1.5 block text-xs font-bold text-cm-danger">处理日志</span>
                  <span class="block max-h-[200px] overflow-auto overscroll-contain break-words font-mono text-xs leading-[1.45] text-cm-primary [user-select:text]">{{ failureLogTitle(file) }}</span>
                </span>
              </span>
              <button v-if="isCompletedStatus(file.status)" :class="getDocumentActionButtonClasses()" title="查看切片" @click="openSlicesDialog(file)">▤</button>
              <button :class="getDocumentActionButtonClasses()" title="下载" @click="downloadFile(file)">⇩</button>
              <button :class="getDocumentActionButtonClasses()" title="重新解析" @click="reparseFile(file)">↻</button>
              <button :class="getDocumentActionButtonClasses()" title="删除" @click="removeFile(file)">⌫</button>
            </div>
          </div>
          <div v-if="!detailLoading && documentTotal > documentPageSize" class="mt-3.5 flex items-center justify-between gap-3 border-t border-cm-divider pt-3.5 text-[13px] text-cm-tertiary">
            <span>第 {{ documentPage }} / {{ documentPageCount }} 页</span>
            <div class="inline-flex items-center gap-2">
              <button type="button" :class="getPaginationButtonClasses()" :disabled="documentPage <= 1" @click="changeDocumentPage(documentPage - 1)">上一页</button>
              <button
                v-for="pageItem in documentPageItems"
                :key="pageItem"
                type="button"
                :class="getPaginationButtonClasses(pageItem === documentPage)"
                @click="changeDocumentPage(pageItem)"
              >
                {{ pageItem }}
              </button>
              <button type="button" :class="getPaginationButtonClasses()" :disabled="documentPage >= documentPageCount" @click="changeDocumentPage(documentPage + 1)">下一页</button>
            </div>
          </div>
        </section>
      </section>
    </main>

    <div v-if="dialogOpen" class="fixed inset-0 z-[10000] flex items-center justify-center bg-[color-mix(in_srgb,var(--cm-bg-canvas)_72%,transparent)]" @click.self="closeDialog">
      <form class="grid w-[min(460px,calc(100vw-32px))] gap-4 rounded-xl border border-cm-strong bg-cm-panel p-6 text-cm-primary shadow-[0_24px_80px_color-mix(in_srgb,var(--cm-bg-canvas)_45%,transparent)]" @submit.prevent="submitKnowledgeBase">
        <h2 class="m-0">{{ dialogMode === 'create' ? '新建知识库' : '知识库设置' }}</h2>
        <label class="flex flex-col gap-2 text-cm-secondary">
          名称
          <input v-model="kbForm.name" class="w-full rounded-lg border border-cm-default bg-cm-input px-3 py-2.5 text-cm-primary outline-none focus:border-cm-focus" required maxlength="50" />
        </label>
        <label class="flex flex-col gap-2 text-cm-secondary">
          描述
          <textarea v-model="kbForm.description" class="w-full resize-y rounded-lg border border-cm-default bg-cm-input px-3 py-2.5 text-cm-primary outline-none focus:border-cm-focus" rows="4" />
        </label>
        <label class="flex items-center gap-2 text-cm-secondary">
          <input v-model="kbForm.is_public" class="w-auto" type="checkbox" />
          全平台公开
        </label>
        <div class="flex justify-end gap-2.5">
          <button type="button" :class="getPanelButtonClasses(false, 'h-[38px] px-[18px]')" @click="closeDialog">取消</button>
          <button type="submit" :class="getPanelButtonClasses(true, 'h-[38px] px-[18px]')" :disabled="actionLoading">{{ actionLoading ? '提交中...' : '保存' }}</button>
        </div>
      </form>
    </div>

    <div v-if="slicesDialogOpen" class="fixed inset-0 z-[10000] flex items-center justify-center bg-[color-mix(in_srgb,var(--cm-bg-canvas)_72%,transparent)]" @click.self="closeSlicesDialog">
      <section class="flex max-h-[min(720px,calc(100vh-48px))] w-[min(760px,calc(100vw-32px))] flex-col gap-4 rounded-xl border border-cm-strong bg-cm-panel p-6 text-cm-primary shadow-[0_24px_80px_color-mix(in_srgb,var(--cm-bg-canvas)_45%,transparent)]">
        <div class="flex items-start justify-between gap-4">
          <div>
            <h2 class="mb-1.5 mt-0">文件切片</h2>
            <p class="m-0 text-cm-tertiary">{{ cleanFilename(activeSliceFile?.filename) }}</p>
          </div>
          <button type="button" class="h-[30px] w-[30px] cursor-pointer rounded-md border border-cm-default bg-cm-surface-muted text-cm-secondary transition hover:bg-cm-accent-soft hover:text-cm-heading" @click="closeSlicesDialog">×</button>
        </div>
        <div v-if="slicesLoading" class="p-6 text-center text-cm-tertiary">加载切片中...</div>
        <div v-else-if="!activeSlices.length" class="p-6 text-center text-cm-tertiary">暂无切片</div>
        <div v-else class="grid min-h-0 gap-2.5 overflow-auto pr-1">
          <article v-for="(slice, index) in activeSlices" :key="slice.id || index" class="rounded-lg border border-cm-default bg-cm-surface-muted px-3.5 py-3">
            <div class="mb-2 flex items-center justify-between gap-3">
              <strong>切片 #{{ slice.index ?? slice.chunk_index ?? slice.slice_index ?? index + 1 }}</strong>
              <span v-if="slice.score != null" class="text-xs text-cm-accent">{{ formatScore(slice.score) }}</span>
            </div>
            <p class="m-0 whitespace-pre-wrap break-words leading-[1.6] text-cm-primary">{{ slice.content || slice.text || slice.chunk || slice.page_content || '无内容' }}</p>
          </article>
        </div>
      </section>
    </div>
    <WikiBrowser
      v-if="wikiScope"
      :kb-id="wikiScope.kbId"
      :title="wikiScope.title"
      :subtitle="wikiScope.subtitle"
      :can-edit="wikiScope.canEdit"
      @close="wikiScope = null"
      @locate-file="() => {}"
    />
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox } from 'element-plus'
import { DataAnalysis, Files, PriceTag, Timer, Loading, CircleCheck, Box, CircleClose, QuestionFilled } from '@element-plus/icons-vue'
import { useAuthStore } from '../stores/authStore'
import { useSystemConfigStore } from '../stores/systemConfigStore'
import {
  createKnowledgeBase,
  deleteKnowledgeBase,
  deleteKnowledgeFile,
  buildKnowledgeFileDownloadUrl,
  getKnowledgeBase,
  getKnowledgeBaseTags,
  getKnowledgeFileSlices,
  listKnowledgeBaseFiles,
  listKnowledgeBases,
  reparseKnowledgeBase,
  searchKnowledge,
  updateKnowledgeFile,
  updateKnowledgeBase,
  uploadKnowledgeFiles,
} from '../apis/knowledge/knowledgeApi'
import { downloadFileByUrl } from '@/utils/download'
import WikiBrowser from '@/components/WikiBrowser.vue'

defineProps({
  embedded: {
    type: Boolean,
    default: false,
  },
})

const router = useRouter()
const authStore = useAuthStore()
const systemConfigStore = useSystemConfigStore()
const systemInfo = computed(() => systemConfigStore.systemInfo)

const knowledgeBases = ref([])
const selectedKb = ref(null)
const KB_PAGE_SIZE = 100
const kbPage = ref(1)
const kbTotal = ref(0)
const selectedDetail = ref(null)
const selectedKbId = ref(null)
const kbFilter = ref('')
const searchQuery = ref('')
const searchResults = ref([])
const listLoading = ref(false)
const detailLoading = ref(false)
const searchLoading = ref(false)
const actionLoading = ref(false)
const isDragging = ref(false)
const fileInputRef = ref(null)
const dialogOpen = ref(false)
const dialogMode = ref('create')
const slicesDialogOpen = ref(false)
const slicesLoading = ref(false)
const activeSliceFile = ref(null)
const activeSlices = ref([])
const kbFiles = ref([])
const kbFileTotal = ref(0)
const channelOverviewFiles = ref([])
const channelOverviewTotal = ref(0)
const channelOverviewTags = ref([])
const channelOverviewLoading = ref(false)
const wikiScope = ref(null)
const activeChannelTag = ref(null)
const documentPage = ref(1)
const documentPageSize = ref(10)
let detailRefreshTimer = null
let detailRefreshToken = 0
const DETAIL_REFRESH_INTERVAL = 3000
const ACTIVE_FILE_STATUSES = new Set([0, 2])
const CHANNEL_OVERVIEW_PAGE_SIZE = 100
const kbForm = reactive({
  name: '',
  description: '',
  is_public: true,
})

const currentUser = computed(() => authStore.user)
const userInitial = computed(() => {
  const text = String(currentUser.value?.username || currentUser.value?.full_name || 'U')
  return text.charAt(0).toUpperCase()
})

const filteredKnowledgeBases = computed(() => {
  const keyword = kbFilter.value.trim().toLowerCase()
  if (!keyword) return knowledgeBases.value
  return knowledgeBases.value.filter((item) => {
    return [item.name, item.description].some((value) => String(value || '').toLowerCase().includes(keyword))
  })
})

const CHANNEL_KB_PREFIX = '[频道]'
const isChannelKb = (item) => String(item?.name || '').startsWith(CHANNEL_KB_PREFIX)
const channelKbName = (name) => String(name || '').replace(/^\[频道\]\s*/, '')
const KNOWLEDGE_TAG_LABELS = Object.freeze({
  conversation: '会话资料',
  migrated: '历史迁移',
  meeting: '会议资料',
  meeting_attachment: '会议附件',
  handover: '任务交接',
  task_summary: '任务复盘',
  '相关预案': '相关预案',
  '预案': '相关预案',
  '通话': '通话',
  '会议': '会议',
  '文书': '文书',
  '地图卡': '地图卡',
  '历史案例': '历史案例',
})

const channelKnowledgeBases = computed(() => filteredKnowledgeBases.value.filter(isChannelKb))
const publicKnowledgeBases = computed(() => filteredKnowledgeBases.value.filter((item) => item.is_public && !isChannelKb(item)))
const privateKnowledgeBases = computed(() => filteredKnowledgeBases.value.filter((item) => !item.is_public && !isChannelKb(item)))
const kbTotalPages = computed(() => Math.max(1, Math.ceil(kbTotal.value / KB_PAGE_SIZE)))
const files = computed(() => kbFiles.value)
const documentTotal = computed(() => kbFileTotal.value)
const documentPageCount = computed(() => Math.max(1, Math.ceil(documentTotal.value / documentPageSize.value)))
const documentPageItems = computed(() => {
  const count = documentPageCount.value
  const current = documentPage.value
  const start = Math.max(1, Math.min(current - 2, count - 4))
  const end = Math.min(count, start + 4)
  return Array.from({ length: end - start + 1 }, (_, index) => start + index)
})
const isSelectedChannelKb = computed(() => isChannelKb(selectedDetail.value || selectedKb.value))
const statusBreakdown = computed(() =>
  extractStatusBreakdown(
    selectedDetail.value,
    isSelectedChannelKb.value && channelOverviewFiles.value.length ? channelOverviewFiles.value : kbFiles.value,
  ),
)
const isSelectedPublic = computed(() => Boolean(selectedDetail.value?.is_public ?? selectedKb.value?.is_public))
const activeChannelTagLabel = computed(() =>
  activeChannelTag.value ? KNOWLEDGE_TAG_LABELS[activeChannelTag.value] || '' : '',
)
const channelOverviewCards = computed(() => {
  return [
    { key: 'all', label: '全部资料', description: '当前频道', count: channelOverviewTotal.value, icon: Files, tag: null },
    ...channelOverviewTags.value.map((item, index) => ({
      key: `tag-${index}`,
      label: item.label,
      description: '知识标签',
      count: item.file_count,
      icon: PriceTag,
      tag: item.tag,
    })),
  ]
})

const getPanelButtonClasses = (primary = false, extra = '') => [
  'inline-flex cursor-pointer items-center justify-center rounded-lg border text-sm transition disabled:cursor-not-allowed disabled:opacity-55',
  primary
    ? 'border-cm-focus bg-cm-accent text-cm-on-accent shadow-cm-glow-soft hover:bg-cm-brand-secondary'
    : 'border-cm-default bg-cm-surface text-cm-accent hover:border-cm-focus hover:text-cm-heading',
  extra,
]

const getKnowledgeItemClasses = (kb) => [
  'group mb-2 grid min-h-[72px] w-full cursor-pointer grid-cols-[24px_minmax(0,1fr)_auto] items-center gap-2.5 rounded-lg border p-3 text-left text-inherit transition',
  selectedKbId.value === kb.id
    ? 'border-[color-mix(in_srgb,var(--cm-brand-primary)_62%,transparent)] bg-[color-mix(in_srgb,var(--cm-brand-primary)_18%,transparent)]'
    : 'border-cm-default bg-cm-surface-muted hover:border-[color-mix(in_srgb,var(--cm-brand-primary)_62%,transparent)] hover:bg-[color-mix(in_srgb,var(--cm-brand-primary)_18%,transparent)]',
]

const getKnowledgeActionClasses = (danger = false) => [
  'inline-flex h-[22px] w-[22px] items-center justify-center rounded border-0 bg-transparent transition hover:bg-cm-surface hover:text-cm-heading',
  danger
    ? 'text-cm-danger hover:bg-[color-mix(in_srgb,var(--cm-status-danger)_15%,transparent)] hover:text-cm-danger'
    : 'text-cm-secondary',
]

const getVisibilityPillClasses = () => [
  'rounded-[7px] border bg-[color-mix(in_srgb,var(--cm-brand-primary)_12%,transparent)] px-2 py-0.5 text-xs',
  isSelectedPublic.value
    ? 'border-[color-mix(in_srgb,var(--cm-brand-primary)_32%,transparent)] text-cm-accent'
    : 'border-[color-mix(in_srgb,var(--cm-task-personnel)_35%,transparent)] text-[var(--cm-task-personnel)]',
]

const getSyncPillClasses = () => [
  'inline-flex flex-none items-center gap-[7px] rounded-md border px-2.5 py-1 text-xs',
  statusBreakdown.value.processing > 0
    ? 'border-[color-mix(in_srgb,var(--cm-status-info)_30%,transparent)] bg-[color-mix(in_srgb,var(--cm-status-info)_10%,transparent)] text-cm-info'
    : 'border-[color-mix(in_srgb,var(--cm-status-success)_30%,transparent)] bg-[color-mix(in_srgb,var(--cm-status-success)_10%,transparent)] text-cm-success',
]

const getOverviewCardClasses = (card) => [
  'flex min-h-[76px] min-w-0 cursor-pointer items-center gap-2.5 rounded-lg border p-3 text-left font-inherit text-inherit transition hover:-translate-y-px hover:border-[color-mix(in_srgb,var(--cm-brand-primary)_48%,transparent)] hover:bg-[color-mix(in_srgb,var(--cm-brand-primary)_14%,var(--cm-bg-surface))]',
  activeChannelTag.value === card.tag
    ? 'border-[color-mix(in_srgb,var(--cm-brand-primary)_76%,transparent)] bg-[linear-gradient(135deg,color-mix(in_srgb,var(--cm-brand-primary)_20%,transparent),color-mix(in_srgb,var(--cm-brand-secondary)_18%,transparent))] shadow-cm-glow-soft'
    : 'border-cm-default bg-cm-surface',
]

const getOverviewIconClasses = (card) => [
  'flex h-[34px] w-[34px] flex-none items-center justify-center rounded-[7px]',
  String(card.key).startsWith('tag-')
    ? 'bg-[color-mix(in_srgb,var(--cm-task-personnel)_10%,transparent)] text-[var(--cm-task-personnel)]'
    : 'bg-[color-mix(in_srgb,var(--cm-brand-primary)_10%,transparent)] text-cm-accent',
]

const getDocumentStatusClasses = (status) => {
  const palette = {
    pending: 'border-[color-mix(in_srgb,var(--cm-status-warning)_40%,transparent)] text-cm-warning',
    skipped: 'border-[color-mix(in_srgb,var(--cm-status-warning)_40%,transparent)] text-cm-warning',
    processing: 'border-[color-mix(in_srgb,var(--cm-status-info)_40%,transparent)] text-cm-info',
    ready: 'border-[color-mix(in_srgb,var(--cm-status-success)_40%,transparent)] text-cm-success',
    failed: 'group border-[color-mix(in_srgb,var(--cm-status-danger)_40%,transparent)] text-cm-danger underline decoration-[color-mix(in_srgb,var(--cm-status-danger)_70%,transparent)] decoration-dotted underline-offset-[3px]',
    muted: 'border-cm-default text-cm-tertiary',
  }
  const tone = statusMeta(status).color
  return [
    'relative inline-flex items-center gap-1 rounded-[7px] border bg-[color-mix(in_srgb,var(--cm-brand-primary)_12%,transparent)] px-2 py-0.5 text-xs',
    palette[tone] || palette.muted,
  ]
}

const getDocumentActionButtonClasses = () =>
  'h-[30px] w-[30px] cursor-pointer rounded-lg border border-cm-default bg-cm-surface text-cm-accent transition hover:border-cm-focus hover:text-cm-heading'

const getPaginationButtonClasses = (active = false) => [
  'h-[30px] min-w-8 cursor-pointer rounded-md border px-2.5 text-cm-accent transition disabled:cursor-not-allowed disabled:opacity-55',
  active
    ? 'border-cm-focus bg-[color-mix(in_srgb,var(--cm-brand-primary)_18%,transparent)] text-cm-heading'
    : 'border-cm-default bg-cm-surface hover:border-cm-focus hover:bg-[color-mix(in_srgb,var(--cm-brand-primary)_18%,transparent)] hover:text-cm-heading',
]

function extractStatusBreakdown(detail, fileItems = []) {
  const summary = detail?.status_breakdown || detail?.file_status_summary || detail?.status_summary
  if (summary && typeof summary === 'object') {
    return {
      completed: Number(summary.completed ?? summary.completed_count ?? 0),
      processing: Number(summary.processing ?? summary.processing_count ?? 0),
      failed: Number(summary.failed ?? summary.failed_count ?? 0),
    }
  }
  const fromDetail = {
    completed: Number(detail?.completed_count ?? detail?.completed ?? NaN),
    processing: Number(detail?.processing_count ?? detail?.processing ?? NaN),
    failed: Number(detail?.failed_count ?? detail?.failed ?? NaN),
  }
  if ([fromDetail.completed, fromDetail.processing, fromDetail.failed].every(Number.isFinite)) {
    return fromDetail
  }
  return buildStatusBreakdown(fileItems)
}

function extractKnowledgeFiles(detail) {
  const candidates = [
    detail?.files,
    detail?.items,
    detail?.results,
    detail?.data?.files,
    detail?.data?.items,
    detail?.files?.items,
    detail?.files?.results,
  ]
  return candidates.find(Array.isArray) || []
}

function buildStatusBreakdown(fileItems = []) {
  return fileItems.reduce(
    (result, file) => {
      const status = Number(file?.status)
      if (status === 1) result.completed += 1
      else if (ACTIVE_FILE_STATUSES.has(status)) result.processing += 1
      else if (status === -1) result.failed += 1
      return result
    },
    { completed: 0, processing: 0, failed: 0 },
  )
}

function hasActiveFiles(fileItems = kbFiles.value) {
  return fileItems.some((file) => ACTIVE_FILE_STATUSES.has(Number(file?.status)))
}

function syncKnowledgeBaseSummary(detail = selectedDetail.value) {
  if (!detail?.id) return
  const normalizedId = String(detail.id)
  const fileCount = Number(detail.file_count ?? extractKnowledgeFiles(detail).length ?? 0)
  const patch = {
    ...detail,
    file_count: Number.isFinite(fileCount) ? fileCount : 0,
  }
  knowledgeBases.value = knowledgeBases.value.map((item) => (String(item.id) === normalizedId ? { ...item, ...patch } : item))
  if (String(selectedKb.value?.id) === normalizedId) {
    selectedKb.value = { ...selectedKb.value, ...patch }
  }
}

function clearDetailRefreshTimer() {
  if (detailRefreshTimer) {
    clearTimeout(detailRefreshTimer)
    detailRefreshTimer = null
  }
}

function scheduleDetailRefresh() {
  clearDetailRefreshTimer()
  if (!selectedKbId.value) return
  if (statusBreakdown.value.processing <= 0 && !hasActiveFiles()) return
  const token = detailRefreshToken
  detailRefreshTimer = setTimeout(async () => {
    if (token !== detailRefreshToken || !selectedKbId.value) return
    await loadKnowledgeDetail(selectedKbId.value, { silent: true })
  }, DETAIL_REFRESH_INTERVAL)
}

async function refreshAll() {
  detailRefreshToken += 1
  clearDetailRefreshTimer()
  await loadKnowledgeBases()
  if (selectedKbId.value) {
    await loadKnowledgeDetail(selectedKbId.value)
  }
}

async function loadKnowledgeBases(page = kbPage.value) {
  listLoading.value = true
  try {
    const res = await listKnowledgeBases({ page, size: KB_PAGE_SIZE })
    const items = Array.isArray(res) ? res : res?.items || []
    knowledgeBases.value = items
    kbPage.value = page
    kbTotal.value = Array.isArray(res) ? items.length : Number(res?.total ?? items.length)
    const clamped = Math.min(page, kbTotalPages.value)
    if (clamped !== page) {
      await loadKnowledgeBases(clamped)
      return
    }
    const stillExists = items.some((item) => String(item.id) === String(selectedKbId.value))
    if (!stillExists && items.length) {
      await selectKnowledgeBase(items[0])
    }
  } catch (error) {
    ElMessage.error(error.message || '知识库列表加载失败')
  } finally {
    listLoading.value = false
  }
}

function changeKbPage(next) {
  const target = Math.min(Math.max(1, next), kbTotalPages.value)
  if (target === kbPage.value || listLoading.value) return
  loadKnowledgeBases(target)
}

async function selectKnowledgeBase(kb) {
  detailRefreshToken += 1
  clearDetailRefreshTimer()
  selectedKb.value = kb
  selectedKbId.value = kb.id
  documentPage.value = 1
  searchResults.value = []
  channelOverviewFiles.value = []
  channelOverviewTotal.value = 0
  channelOverviewTags.value = []
  activeChannelTag.value = null
  await loadKnowledgeDetail(kb.id)
}

async function selectChannelTag(tag) {
  const normalizedTag = tag == null ? null : String(tag).trim()
  if (activeChannelTag.value === normalizedTag || !selectedKbId.value) return
  activeChannelTag.value = normalizedTag
  documentPage.value = 1
  await loadKnowledgeFiles(selectedKbId.value, 1)
}

async function loadKnowledgeDetail(id, options = {}) {
  if (!id) return
  const token = detailRefreshToken
  const silent = Boolean(options.silent)
  if (!silent) detailLoading.value = true
  try {
    const detail = await getKnowledgeBase(id)
    if (String(selectedKbId.value) !== String(id) || token !== detailRefreshToken) return
    selectedDetail.value = detail
    syncKnowledgeBaseSummary(detail)
    const requestedPage = documentPage.value
    await loadKnowledgeFiles(id, requestedPage, { silent: true })
    if (isSelectedChannelKb.value) {
      await loadChannelOverview(id, { silent })
    }
    if (documentPage.value > documentPageCount.value) {
      documentPage.value = Math.max(1, documentPageCount.value)
      if (documentPage.value !== requestedPage) {
        await loadKnowledgeFiles(id, documentPage.value, { silent: true })
      }
    }
    scheduleDetailRefresh()
  } catch (error) {
    if (!silent) ElMessage.error(error.message || '知识库详情加载失败')
    else scheduleDetailRefresh()
  } finally {
    if (!silent) detailLoading.value = false
  }
}

async function loadChannelOverview(id, options = {}) {
  if (!id || !isSelectedChannelKb.value) return
  const token = detailRefreshToken
  if (!options.silent) channelOverviewLoading.value = true
  try {
    const [firstResponse, tagResponse] = await Promise.all([
      listKnowledgeBaseFiles(id, { page: 1, size: CHANNEL_OVERVIEW_PAGE_SIZE }),
      getKnowledgeBaseTags(id),
    ])
    const firstPage = normalizePageResponse(firstResponse, CHANNEL_OVERVIEW_PAGE_SIZE)
    const overviewPageSize = Math.max(1, firstPage.size || CHANNEL_OVERVIEW_PAGE_SIZE)
    const pageCount = Math.max(1, Math.ceil(firstPage.total / overviewPageSize))
    const remainingResponses = pageCount > 1
      ? await Promise.all(
          Array.from({ length: pageCount - 1 }, (_, index) =>
            listKnowledgeBaseFiles(id, { page: index + 2, size: overviewPageSize }),
          ),
        )
      : []
    if (String(selectedKbId.value) !== String(id) || token !== detailRefreshToken) return
    const allItems = [
      ...firstPage.items,
      ...remainingResponses.flatMap((response) => normalizePageResponse(response).items),
    ]
    const uniqueItems = Array.from(new Map(allItems.map((item, index) => [String(item?.id ?? `row-${index}`), item])).values())
    channelOverviewFiles.value = uniqueItems
    channelOverviewTotal.value = firstPage.total
    channelOverviewTags.value = normalizeKnowledgeTags(tagResponse)
  } catch (error) {
    if (!options.silent) ElMessage.error(error.message || '频道知识概览加载失败')
    channelOverviewFiles.value = [...kbFiles.value]
    channelOverviewTotal.value = documentTotal.value
    channelOverviewTags.value = []
  } finally {
    if (!options.silent) channelOverviewLoading.value = false
  }
}

function normalizeKnowledgeTags(response = {}) {
  const items = Array.isArray(response?.tags) ? response.tags : []
  return items
    .map((item) => ({
      tag: String(item?.tag || '').trim(),
      label: KNOWLEDGE_TAG_LABELS[String(item?.tag || '').trim()] || '',
      file_count: Math.max(0, Number(item?.file_count) || 0),
    }))
    .filter((item) => item.tag && item.label)
}

async function changeDocumentPage(page) {
  const nextPage = Math.min(Math.max(1, Number(page) || 1), documentPageCount.value)
  if (nextPage === documentPage.value) return
  documentPage.value = nextPage
  if (selectedKbId.value) {
    await loadKnowledgeFiles(selectedKbId.value, nextPage)
  }
}

function normalizePageResponse(response = {}, fallbackSize = documentPageSize.value) {
  if (Array.isArray(response)) {
    return { items: response, total: response.length, page: 1, size: response.length }
  }
  const candidates = [
    response?.items,
    response?.files,
    response?.results,
    response?.list,
    response?.data?.items,
    response?.data?.files,
    response?.data?.results,
    response?.data?.list,
  ]
  const items = candidates.find(Array.isArray) || []
  const total = Number(
    response?.total
      ?? response?.total_count
      ?? response?.totalCount
      ?? response?.count
      ?? response?.total_items
      ?? response?.totalItems
      ?? items.length,
  )
  return {
    items,
    total: Number.isFinite(total) ? total : items.length,
    page: Number(response?.page ?? response?.current_page ?? response?.currentPage ?? 1),
    size: Number(response?.size ?? response?.page_size ?? response?.pageSize ?? fallbackSize),
  }
}

async function loadKnowledgeFiles(id, page = 1, options = {}) {
  if (!id) return
  const token = detailRefreshToken
  try {
    const response = await listKnowledgeBaseFiles(id, {
      page,
      size: documentPageSize.value,
      ...(activeChannelTag.value ? { tag: activeChannelTag.value } : {}),
    })
    if (String(selectedKbId.value) !== String(id) || token !== detailRefreshToken) return
    const { items, total } = normalizePageResponse(response)
    kbFiles.value = items
    kbFileTotal.value = total
    syncKnowledgeBaseSummary({
      ...selectedDetail.value,
      id,
      file_count: total,
    })
  } catch (error) {
    if (!options.silent) ElMessage.error(error.message || '文档列表加载失败')
  }
}

async function runSearch() {
  const query = searchQuery.value.trim()
  if (!query || !selectedKbId.value) return
  searchLoading.value = true
  try {
    const data = await searchKnowledge({ query, kb_id: String(selectedKbId.value) })
    searchResults.value = Array.isArray(data?.results) ? data.results.slice(0, 8) : []
    if (!searchResults.value.length) ElMessage.info('未检索到相关内容')
  } catch (error) {
    ElMessage.error(error.message || '检索失败')
  } finally {
    searchLoading.value = false
  }
}

function clearSearchResults() {
  searchResults.value = []
}

function openCreateDialog() {
  dialogMode.value = 'create'
  kbForm.name = ''
  kbForm.description = ''
  kbForm.is_public = true
  dialogOpen.value = true
}

async function openEditDialog(kb = null) {
  if (kb && selectedKbId.value !== kb.id) {
    await selectKnowledgeBase(kb)
  }
  if (!selectedKbId.value) return
  dialogMode.value = 'edit'
  kbForm.name = selectedDetail.value?.name || selectedKb.value?.name || ''
  kbForm.description = selectedDetail.value?.description || selectedKb.value?.description || ''
  kbForm.is_public = Boolean(selectedDetail.value?.is_public ?? selectedKb.value?.is_public)
  dialogOpen.value = true
}

function closeDialog() {
  dialogOpen.value = false
}

async function submitKnowledgeBase() {
  actionLoading.value = true
  try {
    const payload = {
      name: kbForm.name.trim(),
      description: kbForm.description.trim(),
      is_public: kbForm.is_public,
      kb_type: 'analysis',
    }
    if (dialogMode.value === 'create') {
      const created = await createKnowledgeBase(payload)
      ElMessage.success('知识库已创建')
      closeDialog()
      await loadKnowledgeBases()
      await selectKnowledgeBase(created)
    } else {
      await updateKnowledgeBase(selectedKbId.value, payload)
      ElMessage.success('知识库已更新')
      closeDialog()
      await refreshAll()
    }
  } catch (error) {
    ElMessage.error(error.message || '保存失败')
  } finally {
    actionLoading.value = false
  }
}

async function removeKnowledgeBase(kb) {
  if (!kb?.id) return
  try {
    await ElMessageBox.confirm(`确定删除「${kb.name || '知识库'}」吗？`, '删除知识库', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
  } catch {
    return
  }

  actionLoading.value = true
  try {
    await deleteKnowledgeBase(kb.id)
    ElMessage.success('知识库已删除')
    if (selectedKbId.value === kb.id) {
      selectedKb.value = null
      selectedDetail.value = null
      selectedKbId.value = null
    }
    await loadKnowledgeBases()
  } catch (error) {
    ElMessage.error(error.message || '删除失败')
  } finally {
    actionLoading.value = false
  }
}

async function uploadFiles(fileList) {
  const picked = Array.from(fileList || [])
  if (!picked.length) return
  if (!selectedKbId.value) {
    ElMessage.warning('请先选择一个知识库')
    return
  }
  actionLoading.value = true
  try {
    await uploadKnowledgeFiles({
      files: picked,
      kbId: selectedKbId.value,
      isPublic: isSelectedPublic.value,
      immediateParse: true,
      sync: false,
    })
    ElMessage.success(`已提交 ${picked.length} 个文件`)
    await loadKnowledgeBases()
    documentPage.value = 1
    await loadKnowledgeDetail(selectedKbId.value)
  } catch (error) {
    ElMessage.error(error.message || '上传失败')
  } finally {
    actionLoading.value = false
    if (fileInputRef.value) fileInputRef.value.value = ''
  }
}

function handleDrop(event) {
  isDragging.value = false
  uploadFiles(event.dataTransfer?.files)
}

function handleFilePicked(event) {
  uploadFiles(event.target.files)
}

async function removeFile(file) {
  try {
    await ElMessageBox.confirm(`确定删除「${cleanFilename(file.filename)}」吗？`, '删除文档', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
  } catch {
    return
  }
  try {
    await deleteKnowledgeFile(file.id)
    ElMessage.success('文档已删除')
    await loadKnowledgeBases()
    if (files.value.length === 1 && documentPage.value > 1) {
      documentPage.value -= 1
    }
    await loadKnowledgeDetail(selectedKbId.value)
  } catch (error) {
    ElMessage.error(error.message || '删除失败')
  }
}

function downloadFile(file) {
  if (!file?.id) return
  downloadFileByUrl(
    buildKnowledgeFileDownloadUrl(file.id),
    cleanFilename(file.filename),
  )
}

async function reparseFile(file) {
  if (!file?.id) return
  actionLoading.value = true
  try {
    await updateKnowledgeFile(file.id, {
      slice_type: file.slice_type,
    })
    ElMessage.success(`已提交重新解析：${cleanFilename(file.filename)}`)
    await loadKnowledgeBases()
    await loadKnowledgeDetail(selectedKbId.value)
  } catch (error) {
    ElMessage.error(error.message || '重新解析失败')
  } finally {
    actionLoading.value = false
  }
}

function extractSlices(payload) {
  if (Array.isArray(payload)) return payload
  const candidates = [
    payload?.slices,
    payload?.chunks,
    payload?.items,
    payload?.results,
    payload?.data?.slices,
    payload?.data?.chunks,
    payload?.data?.items,
    payload?.data?.results,
    payload?.file?.slices,
    payload?.file?.chunks,
    payload?.detail?.slices,
    payload?.detail?.chunks,
  ]
  return candidates.find(Array.isArray) || []
}

async function openSlicesDialog(file) {
  if (!file?.id) return
  activeSliceFile.value = file
  activeSlices.value = extractSlices(file)
  slicesDialogOpen.value = true
  slicesLoading.value = true
  try {
    const data = await getKnowledgeFileSlices(file.id)
    activeSlices.value = extractSlices(data)
  } catch (error) {
    ElMessage.error(error.message || '切片加载失败')
  } finally {
    slicesLoading.value = false
  }
}

function closeSlicesDialog() {
  slicesDialogOpen.value = false
  activeSliceFile.value = null
  activeSlices.value = []
}

async function reparseSelected() {
  if (!selectedKbId.value) return
  actionLoading.value = true
  try {
    const data = await reparseKnowledgeBase(selectedKbId.value)
    ElMessage.success(`已提交解析：${data?.kb_count || 0} 个知识库，${data?.file_count || 0} 个文件`)
    await loadKnowledgeBases()
    await loadKnowledgeDetail(selectedKbId.value)
  } catch (error) {
    ElMessage.error(error.message || '重新解析失败')
  } finally {
    actionLoading.value = false
  }
}

function openWiki() {
  const kb = selectedDetail.value || selectedKb.value
  if (!kb?.id) {
    ElMessage.warning('请先选择一个知识库')
    return
  }
  wikiScope.value = {
    kbId: kb.id,
    title: '知识库 Wiki',
    subtitle: channelKbName(kb.name || ''),
    canEdit: true,
  }
}

function cleanFilename(filename) {
  const text = String(filename || '')
  return decodeURIComponent(text.split('/').pop() || text)
}

function formatSize(size) {
  const value = Number(size || 0)
  if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)}MB`
  if (value >= 1024) return `${(value / 1024).toFixed(0)}KB`
  return `${value}B`
}

function statusMeta(status) {
  switch (Number(status)) {
    case 0:
      return { text: '待处理', color: 'pending', icon: Timer }
    case 2:
      return { text: '处理中', color: 'processing', icon: Loading }
    case 1:
      return { text: '已完成', color: 'ready', icon: CircleCheck }
    case 3:
      return { text: '不解析', color: 'skipped', icon: Box }
    case -1:
      return { text: '处理失败', color: 'failed', icon: CircleClose }
    default:
      return { text: '未知', color: 'muted', icon: QuestionFilled }
  }
}

function stringifyLogValue(value) {
  if (value == null || value === '') return ''
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

function getFailureLog(file) {
  if (Number(file?.status) !== -1) return ''
  const candidates = [
    file?.log,
    file?.parse_log,
    file?.process_log,
    file?.error_log,
    file?.error_message,
    file?.message,
    file?.error,
    file?.metadata?.log,
    file?.metadata?.parse_log,
    file?.metadata?.error_log,
    file?.metadata?.error_message,
    file?.metadata?.error,
  ]
  return candidates.map(stringifyLogValue).find(Boolean) || ''
}

function isFailedStatus(status) {
  return Number(status) === -1
}

function isCompletedStatus(status) {
  return Number(status) === 1
}

function failureLogTitle(file) {
  if (Number(file?.status) !== -1) return ''
  return getFailureLog(file) || '暂无处理日志'
}

function formatScore(score) {
  const value = Number(score || 0)
  return `匹配度 ${(value * 100).toFixed(1)}`
}

onMounted(async () => {
  await authStore.ensureInitialized()
  await loadKnowledgeBases()
})

onUnmounted(() => {
  detailRefreshToken += 1
  clearDetailRefreshTimer()
})
</script>
