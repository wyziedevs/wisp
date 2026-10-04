<!-- @feature form -->
<script setup>
useHead({ title: 'Contact' })
const form = reactive({ name: '', email: '' })
const errors = ref({})

async function send() {
  try {
    await $fetch('/api/contact', { method: 'POST', body: form })
    await navigateTo('/')
  } catch (e) {
    errors.value = e.data?.data?.errors ?? {}
  }
}
</script>

<template>
  <form @submit.prevent="send">
    <label>Name <input v-model="form.name" required minlength="1" maxlength="50" />
      <small v-if="errors.name" class="problem">{{ errors.name }}</small></label>
    <label>Email <input v-model="form.email" type="email" required />
      <small v-if="errors.email" class="problem">{{ errors.email }}</small></label>
    <button>Send</button>
  </form>
</template>
