export interface BlitsenConfig {
  build?: string;
  output: string;
  name?: string;
}

export declare function defineConfig(config: BlitsenConfig): BlitsenConfig;
