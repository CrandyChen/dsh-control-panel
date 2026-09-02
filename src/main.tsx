import ReactDOM from "react-dom/client";
import App from "./App";
import { api } from "./api";
import "./styles.css";

// 不使用 StrictMode：避免 dev 下 mount 效果双触发导致启动期重复执行状态/环境检测。
// 启动时先取一次配置，好让首帧主题（config.theme）在渲染前就确定，避免「先黑后浅」闪烁；
// 失败时静默回退到跟随系统（auto），不阻塞渲染。
async function bootstrap() {
  let initialTheme = "auto";
  try {
    const cfg = await api.getConfig();
    initialTheme = cfg.theme ?? "auto";
  } catch {
    /* 获取失败保持 auto，不阻塞启动 */
  }
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <App initialTheme={initialTheme} />,
  );
}

void bootstrap();
