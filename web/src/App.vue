<template>
  <el-container class="layout" :class="{ dark: isDark }">
    <el-header class="header">
      <div class="brand">📧 email-sync</div>
      <div class="header-right">
        <el-switch
          v-model="isDark"
          inline-prompt
          active-text="🌙"
          inactive-text="☀️"
        />
        <el-button type="primary" :loading="syncing" @click="doSync">
          <el-icon><Refresh /></el-icon>&nbsp;同步
        </el-button>
      </div>
    </el-header>
    <el-container class="body">
      <el-aside width="220px" class="aside">
        <FolderNav
          :folders="state.folders"
          :current="state.curFolder"
          @select="onSelectFolder"
        />
      </el-aside>
      <el-main class="main">
        <MailList
          :messages="state.messages"
          :total="state.total"
          :page="state.page"
          :loading="listLoading"
          @open="onOpenDetail"
          @change-page="onChangePage"
          @search="onSearch"
        />
      </el-main>
    </el-container>
    <MailDetail
      :visible="detailVisible"
      :message="state.detail"
      :loading="detailLoading"
      @close="detailVisible = false"
    />
  </el-container>
</template>

<script setup>
import { reactive, ref, watch, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import { api } from './api'
import FolderNav from './components/FolderNav.vue'
import MailList from './components/MailList.vue'
import MailDetail from './components/MailDetail.vue'

const isDark = ref(false)
const syncing = ref(false)
const listLoading = ref(false)
const detailVisible = ref(false)
const detailLoading = ref(false)

// 暗色模式：切换 html.dark class（Element Plus dark 主题）
watch(isDark, (v) => {
  document.documentElement.classList.toggle('dark', v)
})

const state = reactive({
  folders: [],
  curFolder: '全部',
  search: '',
  page: 1,
  total: 0,
  messages: [],
  detail: null,
})

// 加载文件夹
async function loadFolders() {
  try {
    state.folders = await api.folders()
  } catch (e) {
    ElMessage.error('加载文件夹失败：' + e.message)
  }
}

// 加载列表
async function loadList() {
  listLoading.value = true
  try {
    const params = { page: state.page }
    if (state.curFolder !== '全部') params.folder = state.curFolder
    if (state.search) params.search = state.search
    const data = await api.messages(params)
    state.total = data.total
    state.page = data.page
    state.messages = data.messages
  } catch (e) {
    ElMessage.error('加载邮件失败：' + e.message)
  } finally {
    listLoading.value = false
  }
}

// 选文件夹
function onSelectFolder(folder) {
  state.curFolder = folder
  state.page = 1
  loadList()
}

// 打开详情
async function onOpenDetail(id) {
  detailVisible.value = true
  detailLoading.value = true
  try {
    state.detail = await api.message(id)
  } catch (e) {
    ElMessage.error('加载详情失败：' + e.message)
  } finally {
    detailLoading.value = false
  }
}

// 同步
async function doSync() {
  syncing.value = true
  try {
    const r = await api.sync()
    ElMessage.success(r.message)
    await loadFolders()
    await loadList()
  } catch (e) {
    ElMessage.error('同步失败：' + e.message)
  } finally {
    syncing.value = false
  }
}

// 搜索（父组件转发，防抖在 MailList 内做）
function onSearch(q) {
  state.search = q
  state.page = 1
  loadList()
}

function onChangePage(p) {
  state.page = p
  loadList()
}

onMounted(() => {
  loadFolders()
  loadList()
})
</script>

<style>
:root {
  --el-color-primary: #4c8dff;
}
* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}
html,
body,
#app {
  height: 100%;
}
body {
  font-family: -apple-system, 'PingFang SC', 'Microsoft YaHei', sans-serif;
  background: var(--el-bg-color-page);
  color: var(--el-text-color-primary);
}
.layout {
  height: 100%;
}
.header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid var(--el-border-color);
  background: var(--el-bg-color);
}
.brand {
  font-weight: 700;
  font-size: 17px;
}
.header-right {
  display: flex;
  align-items: center;
  gap: 16px;
}
.body {
  overflow: hidden;
}
.aside {
  border-right: 1px solid var(--el-border-color);
  background: var(--el-bg-color);
  overflow-y: auto;
}
.main {
  padding: 0;
  overflow: auto;
  background: var(--el-bg-color-page);
}
</style>
