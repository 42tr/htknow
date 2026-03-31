# Frontend

前端使用 `Vue 3 + Vite + Tailwind CSS 4`，构建产物输出到 `frontend/dist`，由后端通过 `rust-embed` 内嵌并提供静态资源服务。

## 开发

```bash
npm ci
npm run dev
```

默认开发代理会把 `/api` 转发到 `http://localhost:3000`。

## 构建

```bash
npm run build
```

构建完成后，后端重新编译时会把 `dist/` 打包进二进制。
