import "dotenv/config";
import path from "node:path";
import { defineConfig } from "drizzle-kit";

const dbPath = path.resolve(process.env.DATABASE_PATH || "data/meshkeeper.db");

export default defineConfig({
  schema: "./db/schema.ts",
  out: "./db/migrations",
  dialect: "sqlite",
  dbCredentials: {
    url: dbPath,
  },
});
