import { type RouteConfig, index, route } from '@react-router/dev/routes';
export default [
  index('routes/home.ts'),
  route('list1000', 'routes/list1000.tsx'),
  route('json-big', 'routes/json-big.ts'),
  route('params/:id', 'routes/params.ts'),
] satisfies RouteConfig;
