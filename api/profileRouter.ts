import { z } from "zod";
import { createRouter, publicQuery } from "./middleware";
import { updateUser, workspacesOfUser } from "./queries/users";
import { requireMe } from "./auth";
import { publicUser } from "./lib/password";

export const profileRouter = createRouter({
  get: publicQuery.query(async ({ ctx }) => {
    const me = await requireMe(ctx);
    const workspaces = await workspacesOfUser(me.id);
    // email и googleLinked нужны профилю: он показывает либо кнопку привязки
    // Google, либо уже привязанную почту.
    return {
      ...publicUser(me),
      workspaces,
      email: null as string | null,
      googleLinked: false,
    };
  }),

  update: publicQuery
    .input(
      z.object({
        fullName: z.string().min(1).optional(),
        position: z.string().nullable().optional(),
        phone: z.string().min(5).optional(),
        avatarUrl: z.string().nullable().optional(),
      }),
    )
    .mutation(async ({ ctx, input }) => {
      const me = await requireMe(ctx);
      return updateUser(me.id, input);
    }),

  /** Заглушка: смена пароля (демо-режим, реальной авторизации нет). */
  changePassword: publicQuery
    .input(
      z.object({
        currentPassword: z.string().min(1),
        newPassword: z.string().min(10),
      }),
    )
    .mutation(async () => ({ ok: true, message: "Пароль изменён (демо-режим)" })),
});
