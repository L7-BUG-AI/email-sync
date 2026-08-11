<template>
  <el-drawer
    v-model="visibleModel"
    :with-header="false"
    size="42%"
    class="detail-drawer"
  >
    <div v-loading="loading" class="detail">
      <template v-if="message">
        <h2 class="title">{{ message.subject || '(无主题)' }}</h2>
        <el-descriptions :column="1" size="small" class="meta">
          <el-descriptions-item label="发件人">
            {{ message.from_addr || '-' }}
          </el-descriptions-item>
          <el-descriptions-item label="收件人">
            {{ message.to_addr || '-' }}
          </el-descriptions-item>
          <el-descriptions-item label="日期">
            {{ message.date || '-' }}
          </el-descriptions-item>
          <el-descriptions-item label="文件夹">
            {{ message.folder }}
          </el-descriptions-item>
        </el-descriptions>

        <div v-if="message.has_attachment" class="att">
            <el-button
            type="warning"
            plain
            size="small"
            tag="a"
            :href="`/api/messages/${message.id}/attachments`"
          >
            <el-icon><Download /></el-icon>
            &nbsp;下载附件 ({{ message.att_name || 'tar.zst' }})
          </el-button>
        </div>

        <div class="body">
          <pre v-if="message.body_text" class="text-body">{{ message.body_text }}</pre>
          <div v-else-if="message.body_html" class="html-hint">
            （HTML 邮件，附件可直接下载）
          </div>
          <div v-else class="html-hint">（无正文）</div>
        </div>
      </template>
      <div v-else-if="!loading" class="empty">加载失败或无数据</div>
    </div>
  </el-drawer>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  visible: { type: Boolean, default: false },
  message: { type: Object, default: null },
  loading: { type: Boolean, default: false },
})
const emit = defineEmits(['close'])

// v-model 包装：关闭抽屉时通知父组件
const visibleModel = computed({
  get: () => props.visible,
  set: (v) => {
    if (!v) emit('close')
  },
})
</script>

<style scoped>
.detail {
  height: 100%;
  display: flex;
  flex-direction: column;
}
.title {
  margin-bottom: 12px;
  font-size: 18px;
}
.meta {
  margin-bottom: 12px;
}
.att {
  margin-bottom: 12px;
}
.body {
  flex: 1;
  overflow-y: auto;
  border-top: 1px solid var(--el-border-color);
  padding-top: 12px;
}
.text-body {
  white-space: pre-wrap;
  word-break: break-word;
  font-family: inherit;
  font-size: 14px;
  line-height: 1.7;
}
.html-hint {
  color: var(--el-text-color-secondary);
  font-size: 13px;
}
.empty {
  text-align: center;
  color: var(--el-text-color-secondary);
  margin-top: 40px;
}
</style>
