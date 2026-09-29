export function load({ params, route }) {
  return { slug: params.slug, id: route.id }
}
