<!-- @feature list -->
<script setup>
useHead({ title: 'Items' })
const { data: items } = await useFetch('/api/items')
</script>

<template>
  <ul>
    <li v-for="item in items" :key="item.id">{{ item.name }}: ${{ item.price }}</li>
  </ul>
</template>
