// What tsc and Prettier do not cover: the rules of hooks and effect dependencies, and which way imports go.
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

// The layers under the features (IPC, shared code, state, queries) do not import a feature.
const lower = "src/{ipc,lib,components,state,queries}";
const noFeatures = { group: ["@/features", "@/features/**"], message: "这一层在 features 之下，不引用功能模块。" };
// The mock backend is picked in src/ipc/index.ts only; elsewhere just the tests use its data builders.
const noMocks = { group: ["@/mocks", "@/mocks/**"], message: "mock 只经 src/ipc/index.ts 接入。" };
const mockUsers = ["src/ipc/index.ts", "src/**/*.test.{ts,tsx}"];
// Connect's state (the connection, the store, what it keeps) is under its views and under the features that
// show it: it imports none of them. The views do import each other (the transcript's rows carry Connect's buttons).
const noViews = {
  group: [
    "@/features/transcript/**",
    "@/features/terminal/**",
    "@/features/connect",
    "../composer/**",
    "../requests/**",
    "../rewind/**",
    "../Header",
    "../Model",
    "../Notice",
    "../Settings",
    "../Tasks",
    "../index",
  ],
  message: "connect/state 在界面之下，不引用对话记录、终端窗口与 Connect 的组件。",
};
const restrict = (...patterns) => ({ "no-restricted-imports": ["error", { patterns }] });

export default tseslint.config(
  { ignores: ["dist", "src-tauri", "crates", "target", "src/ipc/bindings.ts"] },
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: { parser: tseslint.parser },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
    },
  },
  // One rule, so for a file the last block that matches it decides: each block lists everything it forbids.
  { files: ["src/**/*.{ts,tsx}"], ignores: ["src/mocks/**", ...mockUsers], rules: restrict(noMocks) },
  { files: [`${lower}/**/*.{ts,tsx}`], ignores: mockUsers, rules: restrict(noFeatures, noMocks) },
  { files: [`${lower}/**/*.test.{ts,tsx}`, "src/ipc/index.ts"], rules: restrict(noFeatures) },
  { files: ["src/features/connect/state/**/*.{ts,tsx}"], ignores: mockUsers, rules: restrict(noMocks, noViews) },
);
