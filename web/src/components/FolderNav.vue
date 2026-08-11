<template>
  <el-menu
    :default-active="current"
    class="folder-menu"
    @select="onSelect"
  >
    <el-menu-item index="全部">
      <el-icon><Folder /></el-icon>
      <span>全部</span>
      <el-badge :value="totalCount" :max="99999" class="badge" />
    </el-menu-item>
    <el-menu-item
      v-for="f in folders"
      :key="f.name"
      :index="f.name"
    >
      <el-icon><FolderOpened /></el-icon>
      <span>{{ f.name }}</span>
      <el-badge :value="f.count" :max="99999" class="badge" />
    </el-menu-item>
  </el-menu>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  folders: { type: Array, default: () => [] },
  current: { type: String, default: '全部' },
})

const emit = defineEmits(['select'])

const totalCount = computed(() =>
  props.folders.reduce((sum, f) => sum + f.count, 0)
)

function onSelect(index) {
  emit('select', index)
}
</script>

<style scoped>
.folder-menu {
  border-right: none;
}
.folder-menu .el-menu-item {
  display: flex;
  align-items: center;
}
.badge {
  margin-left: auto;
}
</style>
