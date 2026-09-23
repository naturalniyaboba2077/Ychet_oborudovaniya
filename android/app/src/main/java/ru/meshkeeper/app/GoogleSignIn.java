package ru.meshkeeper.app;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
import android.widget.Toast;

import androidx.browser.customtabs.CustomTabsIntent;

import org.json.JSONObject;

import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.SecureRandom;

/**
 * Вход через Google из приложения.
 *
 * Внутри WebView Google вход не пускает: показывает «403
 * disallowed_useragent». Поэтому переход на Google перехватывается и
 * открывается в настоящем браузере (Custom Tab). Но cookie сессии, которую
 * выдаст сервер после входа, досталась бы браузеру, а не приложению.
 *
 * Поэтому перед уходом приложение придумывает одноразовый секрет и
 * сообщает серверу его хэш вместе с `state` попытки (/auth/google/app).
 * После входа сервер уводит браузер обратно в приложение с одноразовым
 * кодом, а приложение открывает в своём WebView /auth/google/app-finish с
 * кодом и секретом — там сессия и ложится в cookie приложения. Код без
 * секрета бесполезен, так что перехват ссылки ничего не даёт (схема PKCE).
 */
final class GoogleSignIn {
    static final String RETURN_SCHEME = "ru.meshkeeper.app";
    static final String RETURN_HOST = "google";

    private static final String PREFS = "meshkeeper-google";
    private static final String KEY_VERIFIER = "verifier";
    private static final String KEY_ORIGIN = "origin";

    private GoogleSignIn() {}

    /** Начало входа на Google: ровно тот адрес, который выдаёт сервер (google.rs). */
    static boolean isAuthStart(Uri uri) {
        return uri != null
                && "https".equalsIgnoreCase(uri.getScheme())
                && "accounts.google.com".equalsIgnoreCase(uri.getHost())
                && uri.getPath() != null
                && uri.getPath().startsWith("/o/oauth2/")
                && uri.getQueryParameter("state") != null;
    }

    static void start(Activity activity, String origin, Uri googleUrl) {
        String state = googleUrl.getQueryParameter("state");
        byte[] raw = new byte[32];
        new SecureRandom().nextBytes(raw);
        String verifier = hex(raw);
        // Процесс может быть убит, пока человек в браузере, — секрет
        // храним на диске, а не в памяти.
        prefs(activity).edit()
                .putString(KEY_VERIFIER, verifier)
                .putString(KEY_ORIGIN, origin)
                .apply();
        new Thread(() -> {
            String error = null;
            try {
                mark(origin, state, sha256(verifier));
            } catch (Exception e) {
                error = e.getMessage();
            }
            String failed = error;
            activity.runOnUiThread(() -> {
                if (failed != null) {
                    Toast.makeText(activity, "Не удалось начать вход через Google: " + failed,
                            Toast.LENGTH_LONG).show();
                    return;
                }
                try {
                    new CustomTabsIntent.Builder().build().launchUrl(activity, googleUrl);
                } catch (Exception e) {
                    // Нет браузера с Custom Tabs — обычный браузер тоже годится.
                    try {
                        activity.startActivity(new Intent(Intent.ACTION_VIEW, googleUrl));
                    } catch (Exception ignored) {
                        Toast.makeText(activity, "Не найден браузер для входа через Google",
                                Toast.LENGTH_LONG).show();
                    }
                }
            });
        }, "mk-google").start();
    }

    /**
     * Возвращение из браузера. Возвращает адрес, который надо открыть в
     * WebView, чтобы завершить вход, или null, если завершать нечего
     * (вход не удался — браузер уже показал причину).
     */
    static String finishUrl(Context ctx, Uri data) {
        if (data == null || !RETURN_SCHEME.equals(data.getScheme())
                || !RETURN_HOST.equals(data.getHost())) return null;
        SharedPreferences p = prefs(ctx);
        String verifier = p.getString(KEY_VERIFIER, null);
        String origin = p.getString(KEY_ORIGIN, null);
        String code = data.getQueryParameter("code");
        if (code == null || verifier == null || origin == null) return null;
        // Секрет одноразовый, как и код.
        p.edit().clear().apply();
        return Uri.parse(origin + "/auth/google/app-finish").buildUpon()
                .appendQueryParameter("code", code)
                .appendQueryParameter("verifier", verifier)
                .build().toString();
    }

    static boolean isReturn(Uri data) {
        return data != null && RETURN_SCHEME.equals(data.getScheme())
                && RETURN_HOST.equals(data.getHost());
    }

    private static void mark(String origin, String state, String challenge) throws Exception {
        URL url = new URL(origin + "/auth/google/app");
        if (!"https".equalsIgnoreCase(url.getProtocol())) throw new Exception("только HTTPS");
        HttpURLConnection c = (HttpURLConnection) url.openConnection();
        try {
            c.setConnectTimeout(15000);
            c.setReadTimeout(15000);
            c.setRequestMethod("POST");
            c.setDoOutput(true);
            c.setRequestProperty("Content-Type", "application/json");
            byte[] body = new JSONObject()
                    .put("state", state)
                    .put("challenge", challenge)
                    .toString().getBytes(StandardCharsets.UTF_8);
            try (OutputStream out = c.getOutputStream()) {
                out.write(body);
            }
            int status = c.getResponseCode();
            if (status == 404) throw new Exception("попытка входа устарела, нажмите ещё раз");
            if (status / 100 != 2) throw new Exception("сервер ответил " + status);
        } finally {
            c.disconnect();
        }
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    private static String sha256(String s) throws Exception {
        return hex(MessageDigest.getInstance("SHA-256").digest(s.getBytes(StandardCharsets.UTF_8)));
    }

    private static String hex(byte[] bytes) {
        StringBuilder out = new StringBuilder();
        for (byte b : bytes) out.append(String.format("%02x", b));
        return out.toString();
    }
}
