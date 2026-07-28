CREATE TABLE IF NOT EXISTS "User" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "username" TEXT NOT NULL,
    "displayName" TEXT,
    "email" TEXT NOT NULL,
    "password" TEXT NOT NULL,
    "avatar" TEXT,
    "banner" TEXT,
    "status" TEXT NOT NULL DEFAULT 'online',
    "bio" TEXT NOT NULL DEFAULT 'Привет! Я использую Akami.',
    "pronouns" TEXT NOT NULL DEFAULT '',
    "profileColor" TEXT NOT NULL DEFAULT '',
    "badges" TEXT NOT NULL DEFAULT '',
    "customStatus" TEXT NOT NULL DEFAULT '',
    "socials" TEXT NOT NULL DEFAULT '',
    "isBot" BOOLEAN NOT NULL DEFAULT 0,
    "ownerId" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL
);

CREATE TABLE IF NOT EXISTS "Server" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "imageUrl" TEXT,
    -- Wide image shown behind the server's name in the channel list.
    "bannerUrl" TEXT,
    "inviteCode" TEXT NOT NULL,
    "ownerId" TEXT NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "Server_ownerId_fkey" FOREIGN KEY ("ownerId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE TABLE IF NOT EXISTS "ServerMember" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "role" TEXT NOT NULL DEFAULT 'GUEST',
    "userId" TEXT NOT NULL,
    "serverId" TEXT NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "ServerMember_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "ServerMember_serverId_fkey" FOREIGN KEY ("serverId") REFERENCES "Server" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE TABLE IF NOT EXISTS "Channel" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "name" TEXT NOT NULL,
    "type" TEXT NOT NULL DEFAULT 'TEXT',
    "serverId" TEXT NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "Channel_serverId_fkey" FOREIGN KEY ("serverId") REFERENCES "Server" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE TABLE IF NOT EXISTS "Message" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "content" TEXT NOT NULL,
    "components" TEXT,
    "userId" TEXT NOT NULL,
    "channelId" TEXT NOT NULL,
    -- The message this one answers, if any.
    "replyToId" TEXT,
    -- When it was pinned; null means it is not.
    "pinnedAt" DATETIME,
    -- Author of the message this one was forwarded from, if it was.
    "forwardedFrom" TEXT,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "Message_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "Message_channelId_fkey" FOREIGN KEY ("channelId") REFERENCES "Channel" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE TABLE IF NOT EXISTS "DirectMessage" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "content" TEXT NOT NULL,
    "senderId" TEXT NOT NULL,
    "receiverId" TEXT NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- Set only when the message is edited; null means it never was.
    "updatedAt" DATETIME,
    -- The message this one answers, if any.
    "replyToId" TEXT,
    -- When it was pinned; null means it is not.
    "pinnedAt" DATETIME,
    -- Author of the message this one was forwarded from, if it was.
    "forwardedFrom" TEXT,
    CONSTRAINT "DirectMessage_senderId_fkey" FOREIGN KEY ("senderId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "DirectMessage_receiverId_fkey" FOREIGN KEY ("receiverId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE TABLE IF NOT EXISTS "Friendship" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "status" TEXT NOT NULL DEFAULT 'PENDING',
    "senderId" TEXT NOT NULL,
    "receiverId" TEXT NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" DATETIME NOT NULL,
    CONSTRAINT "Friendship_senderId_fkey" FOREIGN KEY ("senderId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "Friendship_receiverId_fkey" FOREIGN KEY ("receiverId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS "User_username_key" ON "User"("username");
CREATE UNIQUE INDEX IF NOT EXISTS "User_email_key" ON "User"("email");
CREATE UNIQUE INDEX IF NOT EXISTS "Server_inviteCode_key" ON "Server"("inviteCode");
CREATE UNIQUE INDEX IF NOT EXISTS "ServerMember_userId_serverId_key" ON "ServerMember"("userId", "serverId");
CREATE UNIQUE INDEX IF NOT EXISTS "Friendship_senderId_receiverId_key" ON "Friendship"("senderId", "receiverId");

-- Reactions to a message. `messageId` points at either a Message or a
-- DirectMessage, so there is no foreign key; the delete handlers clear the rows.
CREATE TABLE IF NOT EXISTS "Reaction" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "messageId" TEXT NOT NULL,
    "userId" TEXT NOT NULL,
    "emoji" TEXT NOT NULL,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT "Reaction_userId_fkey" FOREIGN KEY ("userId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

-- One reaction of a given emoji per person per message.
CREATE UNIQUE INDEX IF NOT EXISTS "Reaction_message_user_emoji" ON "Reaction" ("messageId", "userId", "emoji");
CREATE INDEX IF NOT EXISTS "Reaction_messageId" ON "Reaction" ("messageId");

-- Invite links to a server. A code can be one-off (`maxUses` = 1), unlimited
-- (`maxUses` NULL), time-limited (`expiresAt`), or any combination; `revokedAt`
-- turns one off by hand. The legacy `Server.inviteCode` still works alongside.
CREATE TABLE IF NOT EXISTS "Invite" (
    "id" TEXT NOT NULL PRIMARY KEY,
    "code" TEXT NOT NULL,
    "serverId" TEXT NOT NULL,
    "creatorId" TEXT NOT NULL,
    -- NULL means no limit on how many people may use it.
    "maxUses" INTEGER,
    "uses" INTEGER NOT NULL DEFAULT 0,
    -- NULL means it never expires.
    "expiresAt" DATETIME,
    -- Set when someone turns the link off.
    "revokedAt" DATETIME,
    "createdAt" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT "Invite_serverId_fkey" FOREIGN KEY ("serverId") REFERENCES "Server" ("id") ON DELETE CASCADE ON UPDATE CASCADE,
    CONSTRAINT "Invite_creatorId_fkey" FOREIGN KEY ("creatorId") REFERENCES "User" ("id") ON DELETE CASCADE ON UPDATE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS "Invite_code_key" ON "Invite" ("code");
CREATE INDEX IF NOT EXISTS "Invite_serverId" ON "Invite" ("serverId");
