-- Create ModelConfig table
CREATE TABLE IF NOT EXISTS "ModelConfig" (
    id TEXT NOT NULL PRIMARY KEY,
    provider TEXT NOT NULL,
    "baseUrl" TEXT NOT NULL,
    "promptManagerId" TEXT,
    "modelName" TEXT NOT NULL,
    parameters JSON NOT NULL,
    "modelClassName" TEXT NOT NULL,
    "contextLimit" INTEGER NOT NULL,
    live BOOLEAN NOT NULL,
    display BOOLEAN NOT NULL,
    "displayLabel" TEXT,
    "displayIcon" TEXT,
    "displayDescription" TEXT,
    "displayUrl" TEXT,
    "displayCost" TEXT,
    "displayMaxTokens" INTEGER,
    "displayTags" TEXT[],
    "displayParams" TEXT[],
    "modelRetryConfigId" TEXT,
    "modelProviderGroup" TEXT,
    "huggingFaceModelName" TEXT,
    "apiKeyEnvName" TEXT
);

-- Create ModelProvider table
CREATE TABLE IF NOT EXISTS "ModelProvider" (
    id TEXT NOT NULL PRIMARY KEY,
    "group" TEXT NOT NULL,
    provider TEXT NOT NULL,
    weight INTEGER NOT NULL,
    "replicaCount" TEXT NOT NULL,
    "baseUrl" TEXT NOT NULL,
    "maxRps" INTEGER NOT NULL,
    "minRps" INTEGER NOT NULL,
    rps INTEGER,
    active BOOLEAN DEFAULT FALSE NOT NULL,
    "createdAt" TIMESTAMP WITH TIME ZONE DEFAULT NOW() NOT NULL,
    "updatedAt" TIMESTAMP WITH TIME ZONE DEFAULT NOW(),
    tier INTEGER DEFAULT 0 NOT NULL,
    "apiKeyEnvName" TEXT
);

-- Create indexes for better performance
CREATE INDEX IF NOT EXISTS idx_model_config_live ON "ModelConfig"(live);
CREATE INDEX IF NOT EXISTS idx_model_config_display ON "ModelConfig"(display);
CREATE INDEX IF NOT EXISTS idx_model_config_provider_group ON "ModelConfig"("modelProviderGroup");

CREATE INDEX IF NOT EXISTS idx_model_provider_group ON "ModelProvider"("group");
CREATE INDEX IF NOT EXISTS idx_model_provider_active ON "ModelProvider"(active);
CREATE INDEX IF NOT EXISTS idx_model_provider_group_active ON "ModelProvider"("group", active);
CREATE INDEX IF NOT EXISTS idx_model_provider_weight ON "ModelProvider"(weight);
