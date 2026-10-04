<!-- @feature search -->
<script setup>
useHead({ title: 'Search' })
const { data: items } = await useFetch('/api/items')
const q = ref('')
const found = computed(() => items.value.filter((i) => i.name.toLowerCase().includes(q.value.toLowerCase())))
</script>

<template>
  <input v-model="q" placeholder="Search" />
  <ul>
    <li v-for="item in found" :key="item.id">{{ item.name }}</li>
  </ul>
</template>
