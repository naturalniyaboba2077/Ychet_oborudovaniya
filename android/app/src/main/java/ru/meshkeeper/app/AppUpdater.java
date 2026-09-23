package ru.meshkeeper.app;

import android.app.Activity;
import android.app.AlertDialog;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.security.MessageDigest;

/**
 * Самообновление оболочки приложения.
 *
 * Интерфейс обновляется на лету сам: он грузится с сервера, и сервер
 * оповещает открытые страницы о новой сборке (update.rs). А вот сама
 * оболочка — этот Java-код — меняется только установкой нового APK. Сервер
 * отдаёт последний APK на /api/app/android (apk.rs); здесь мы его находим,
 * скачиваем в фоне и предлагаем поставить.
 *
 * Совсем молча Android ставить не даёт: первое обновление всегда
 * подтверждает человек. Начиная с Android 12 последующие проходят без
 * вопросов — приложение становится «установщиком» самого себя, и система
 * разрешает ему обновляться самостоятельно (setRequireUserAction).
 *
 * Подлинность проверяет система: APK с чужой подписью поверх нашего она не
 * установит. Контрольная сумма нужна только против оборванной загрузки.
 */
final class AppUpdater {
    static final String ACTION_STATUS = "ru.meshkeeper.app.UPDATE_STATUS";

    /** Не чаще раза в час: проверка идёт и при каждом возвращении в приложение. */
    private static final long CHECK_EVERY_MS = 60 * 60 * 1000L;
    private static volatile long lastCheck;
    private static volatile boolean busy;
    /** Версия, от которой человек отказался в этом запуске: не надоедаем. */
    private static volatile long declined;

    private AppUpdater() {}

    static void check(Activity activity, String origin) {
        if (origin == null || origin.isEmpty()) return;
        long now = System.currentTimeMillis();
        synchronized (AppUpdater.class) {
            if (busy || now - lastCheck < CHECK_EVERY_MS) return;
            busy = true;
            lastCheck = now;
        }
        Context app = activity.getApplicationContext();
        new Thread(() -> {
            try {
                File apk = fetch(app, origin);
                if (apk != null) activity.runOnUiThread(() -> offer(activity, apk));
            } catch (Exception ignored) {
                // Нет связи или сервер без APK — попробуем при следующем открытии.
                lastCheck = 0;
            } finally {
                busy = false;
            }
        }, "mk-update").start();
    }

    /** Скачивает новую версию, если она есть. null — обновляться не нужно. */
    private static File fetch(Context ctx, String origin) throws Exception {
        JSONObject info = new JSONObject(new String(get(origin + "/api/app/android"), "UTF-8"));
        long code = info.getLong("versionCode");
        if (code <= BuildConfig.VERSION_CODE || code == declined) return null;
        String sha = info.getString("sha256");

        File dir = new File(ctx.getCacheDir(), "update");
        File apk = new File(dir, "meshkeeper-" + code + ".apk");
        if (apk.isFile() && sha.equalsIgnoreCase(sha256(apk))) return apk;

        // Прежние загрузки больше не нужны.
        File[] old = dir.listFiles();
        if (old != null) for (File f : old) f.delete();
        dir.mkdirs();

        File part = new File(dir, apk.getName() + ".part");
        HttpURLConnection c = open(new URL(new URL(origin), info.getString("url")));
        try (InputStream in = c.getInputStream(); OutputStream out = new FileOutputStream(part)) {
            copy(in, out);
        } finally {
            c.disconnect();
        }
        if (!sha.equalsIgnoreCase(sha256(part))) {
            part.delete();
            throw new Exception("контрольная сумма APK не сошлась");
        }
        if (!part.renameTo(apk)) throw new Exception("не удалось сохранить APK");
        return apk;
    }

    private static void offer(Activity activity, File apk) {
        if (activity.isFinishing()) return;
        long code = codeOf(apk);
        new AlertDialog.Builder(activity)
                .setTitle("Доступна новая версия приложения")
                .setMessage("Обновление уже загружено. Установка займёт несколько секунд, "
                        + "приложение перезапустится.")
                .setPositiveButton("Установить", (d, w) -> install(activity, apk))
                .setNegativeButton("Позже", (d, w) -> declined = code)
                .show();
    }

    static void install(Activity activity, File apk) {
        // Разрешение «устанавливать из этого источника» человек даёт один
        // раз в настройках. Без него сессия установки просто не начнётся.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
                && !activity.getPackageManager().canRequestPackageInstalls()) {
            new AlertDialog.Builder(activity)
                    .setTitle("Нужно разрешение")
                    .setMessage("Чтобы приложение могло обновляться само, разрешите ему "
                            + "установку приложений. Это нужно сделать один раз.")
                    .setPositiveButton("Открыть настройки", (d, w) -> activity.startActivity(
                            new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                                    Uri.parse("package:" + activity.getPackageName()))))
                    .setNegativeButton("Отмена", null)
                    .show();
            // После возврата из настроек предложим снова.
            lastCheck = 0;
            return;
        }
        try {
            PackageInstaller installer = activity.getPackageManager().getPackageInstaller();
            PackageInstaller.SessionParams params =
                    new PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL);
            params.setAppPackageName(activity.getPackageName());
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED);
            }
            int id = installer.createSession(params);
            try (PackageInstaller.Session session = installer.openSession(id)) {
                try (InputStream in = new FileInputStream(apk);
                     OutputStream out = session.openWrite("base.apk", 0, apk.length())) {
                    copy(in, out);
                    session.fsync(out);
                }
                Intent status = new Intent(activity, UpdateReceiver.class).setAction(ACTION_STATUS);
                int flags = PendingIntent.FLAG_UPDATE_CURRENT;
                // Установщик дописывает в интент статус, поэтому он изменяемый.
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) flags |= PendingIntent.FLAG_MUTABLE;
                PendingIntent pi = PendingIntent.getBroadcast(activity, id, status, flags);
                session.commit(pi.getIntentSender());
            }
        } catch (Exception e) {
            android.widget.Toast.makeText(activity, "Не удалось начать установку: " + e.getMessage(),
                    android.widget.Toast.LENGTH_LONG).show();
        }
    }

    private static long codeOf(File apk) {
        String n = apk.getName();
        try {
            return Long.parseLong(n.substring("meshkeeper-".length(), n.length() - ".apk".length()));
        } catch (Exception e) {
            return 0;
        }
    }

    private static HttpURLConnection open(URL url) throws Exception {
        if (!"https".equalsIgnoreCase(url.getProtocol())) throw new Exception("только HTTPS");
        HttpURLConnection c = (HttpURLConnection) url.openConnection();
        c.setConnectTimeout(15000);
        c.setReadTimeout(30000);
        c.setUseCaches(false);
        if (c.getResponseCode() != 200) {
            c.disconnect();
            throw new Exception("HTTP " + c.getResponseCode());
        }
        return c;
    }

    private static byte[] get(String url) throws Exception {
        HttpURLConnection c = open(new URL(url));
        try (InputStream in = c.getInputStream()) {
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            copy(in, out);
            return out.toByteArray();
        } finally {
            c.disconnect();
        }
    }

    private static void copy(InputStream in, OutputStream out) throws Exception {
        byte[] buf = new byte[64 * 1024];
        int n;
        while ((n = in.read(buf)) > 0) out.write(buf, 0, n);
    }

    private static String sha256(File f) throws Exception {
        MessageDigest md = MessageDigest.getInstance("SHA-256");
        try (InputStream in = new FileInputStream(f)) {
            byte[] buf = new byte[64 * 1024];
            int n;
            while ((n = in.read(buf)) > 0) md.update(buf, 0, n);
        }
        StringBuilder hex = new StringBuilder();
        for (byte b : md.digest()) hex.append(String.format("%02x", b));
        return hex.toString();
    }
}
