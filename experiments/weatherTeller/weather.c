/*
 * weather.c - CLI weather tool using wttr.in (no API key required)
 *
 * Usage: weather <zip_code> [-f|-c]
 *   -f  Show temperature in Fahrenheit (default)
 *   -c  Show temperature in Celsius
 *
 * Requires: curl (command-line tool, not libcurl)
 * Service:  https://wttr.in  (free, no account needed)
 */

#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>

#define BUFFER_SIZE (128 * 1024)
#define MAX_FIELD   256
#define URL_SIZE    512

/* ------------------------------------------------------------------ */
/* Minimal JSON field extractor                                        */
/* Finds the first occurrence of  "key": "value"  or  "key": number   */
/* and copies the value into out (up to out_size-1 chars).            */
/* Returns 1 on success, 0 if not found.                              */
/* ------------------------------------------------------------------ */
static int json_str(const char *json, const char *key, char *out, int out_size)
{
    char needle[MAX_FIELD];
    snprintf(needle, sizeof(needle), "\"%s\"", key);

    const char *p = strstr(json, needle);
    if (!p) return 0;

    p += strlen(needle);

    /* skip whitespace and colon */
    while (*p && (isspace((unsigned char)*p) || *p == ':'))
        p++;

    if (*p == '"') {
        /* string value */
        p++;
        int i = 0;
        while (*p && *p != '"' && i < out_size - 1)
            out[i++] = *p++;
        out[i] = '\0';
    } else {
        /* numeric / bare value */
        int i = 0;
        while (*p && *p != ',' && *p != '}' && *p != '\n' && i < out_size - 1)
            out[i++] = *p++;
        /* trim trailing whitespace */
        while (i > 0 && isspace((unsigned char)out[i-1]))
            i--;
        out[i] = '\0';
    }
    return 1;
}

/* Like json_str but searches starting from position `from` */
static int json_str_from(const char *from, const char *key,
                         char *out, int out_size)
{
    return json_str(from, key, out, out_size);
}

/* ------------------------------------------------------------------ */
/* Weather code -> description mapping                                 */
/* From https://www.worldweatheronline.com/developer/api/docs/weather-icons.aspx */
/* ------------------------------------------------------------------ */
static const char *weather_code_desc(int code)
{
    switch (code) {
        case 113: return "Sunny / Clear";
        case 116: return "Partly Cloudy";
        case 119: return "Cloudy";
        case 122: return "Overcast";
        case 143: return "Mist";
        case 176: return "Patchy Rain";
        case 179: return "Patchy Snow";
        case 182: return "Patchy Sleet";
        case 185: return "Patchy Freezing Drizzle";
        case 200: return "Thundery Outbreaks";
        case 227: return "Blowing Snow";
        case 230: return "Blizzard";
        case 248: return "Fog";
        case 260: return "Freezing Fog";
        case 263: return "Light Drizzle";
        case 266: return "Drizzle";
        case 281: return "Freezing Drizzle";
        case 284: return "Heavy Freezing Drizzle";
        case 293: return "Light Rain";
        case 296: return "Rain";
        case 299: return "Moderate Rain";
        case 302: return "Heavy Rain";
        case 305: return "Heavy Rain";
        case 308: return "Very Heavy Rain";
        case 311: return "Light Freezing Rain";
        case 314: return "Moderate Freezing Rain";
        case 317: return "Light Sleet";
        case 320: return "Moderate Sleet";
        case 323: return "Light Snow";
        case 326: return "Snow";
        case 329: return "Moderate Snow";
        case 332: return "Heavy Snow";
        case 335: return "Heavy Snow";
        case 338: return "Very Heavy Snow";
        case 350: return "Ice Pellets";
        case 353: return "Light Rain Shower";
        case 356: return "Moderate Rain Shower";
        case 359: return "Torrential Rain";
        case 362: return "Light Sleet Shower";
        case 365: return "Moderate Sleet Shower";
        case 368: return "Light Snow Shower";
        case 371: return "Moderate Snow Shower";
        case 374: return "Light Ice Pellet Shower";
        case 377: return "Moderate Ice Pellet Shower";
        case 386: return "Light Thundery Rain";
        case 389: return "Moderate Thundery Rain";
        case 392: return "Light Thundery Snow";
        case 395: return "Heavy Thundery Snow";
        default:  return "Unknown";
    }
}

/* ASCII art for broad weather categories */
static void print_ascii_art(int code)
{
    if (code == 113) {
        puts("    \\   /    ");
        puts("     .-.     ");
        puts("  ― (   ) ―  ");
        puts("     `-'     ");
        puts("    /   \\    ");
    } else if (code == 116) {
        puts("   \\  /      ");
        puts(" _ /\"\".-'    ");
        puts("   \\_(       ");
        puts("   /  (      ");
        puts("      '-'    ");
    } else if (code == 119 || code == 122) {
        puts("             ");
        puts("     .--.    ");
        puts("  .-(    ).  ");
        puts(" (___.__)__) ");
        puts("             ");
    } else if (code == 248 || code == 260 || code == 143) {
        puts("             ");
        puts(" _ - _ - _ - ");
        puts("  _ - _ - _  ");
        puts(" _ - _ - _ - ");
        puts("             ");
    } else if (code >= 293 && code <= 308) {
        puts("     .--.    ");
        puts("  .-(    ).  ");
        puts(" (___.__)__) ");
        puts("  ' ' ' ' '  ");
        puts(" ' ' ' ' '   ");
    } else if (code >= 323 && code <= 338) {
        puts("     .--.    ");
        puts("  .-(    ).  ");
        puts(" (___.__)__) ");
        puts("  *  *  *  * ");
        puts("   *  *  *   ");
    } else if (code >= 386 && code <= 395) {
        puts("     .--.    ");
        puts("  .-(    ).  ");
        puts(" (___.__)__) ");
        puts("  /_/_/_ /   ");
        puts("   /_/_/     ");
    } else {
        puts("   .-..--.   ");
        puts(" .( o     ). ");
        puts("(___.___.___)");
        puts("             ");
        puts("             ");
    }
}

/* ------------------------------------------------------------------ */

static void usage(const char *prog)
{
    fprintf(stderr, "Usage: %s <zip_code> [-f|-c]\n", prog);
    fprintf(stderr, "  -f  Fahrenheit (default)\n");
    fprintf(stderr, "  -c  Celsius\n");
    fprintf(stderr, "\nExample: %s 90210\n", prog);
}

static int only_digits(const char *s)
{
    if (!*s) return 0;
    for (; *s; s++)
        if (!isdigit((unsigned char)*s)) return 0;
    return 1;
}

int main(int argc, char *argv[])
{
    if (argc < 2) { usage(argv[0]); return 1; }

    const char *zip = NULL;
    int use_celsius = 0;

    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "-c") == 0)      use_celsius = 1;
        else if (strcmp(argv[i], "-f") == 0) use_celsius = 0;
        else if (argv[i][0] != '-')           zip = argv[i];
        else { fprintf(stderr, "Unknown option: %s\n", argv[i]); usage(argv[0]); return 1; }
    }

    if (!zip) { fprintf(stderr, "No zip code specified.\n"); usage(argv[0]); return 1; }
    if (!only_digits(zip) || strlen(zip) < 3) {
        fprintf(stderr, "Invalid zip code: %s\n", zip); return 1;
    }

    /* Build URL */
    char url[URL_SIZE];
    snprintf(url, sizeof(url),
             "https://wttr.in/%s?format=j1", zip);

    /* Build curl command */
    char cmd[URL_SIZE + 128];
    snprintf(cmd, sizeof(cmd),
             "curl -sS --max-time 10 \"%s\"", url);

    /* Fetch */
    FILE *fp = popen(cmd, "r");
    if (!fp) {
        perror("popen");
        return 1;
    }

    char *buf = malloc(BUFFER_SIZE);
    if (!buf) { fputs("Out of memory\n", stderr); pclose(fp); return 1; }

    size_t total = 0;
    size_t n;
    while ((n = fread(buf + total, 1, BUFFER_SIZE - total - 1, fp)) > 0) {
        total += n;
        if (total >= BUFFER_SIZE - 1) break;
    }
    buf[total] = '\0';

    int rc = pclose(fp);
    if (rc != 0 || total == 0) {
        fprintf(stderr, "Failed to fetch weather data (curl exit %d).\n"
                        "Check your internet connection or try again.\n", rc);
        free(buf);
        return 1;
    }

    /* Quick sanity: expect JSON */
    if (buf[0] != '{') {
        fprintf(stderr, "Unexpected response:\n%.*s\n", 200, buf);
        free(buf);
        return 1;
    }

    /* ---- Parse ---------------------------------------------------- */
    char temp_c[16]="?", temp_f[16]="?";
    char feels_c[16]="?", feels_f[16]="?";
    char humidity[16]="?", cloudcover[16]="?";
    char pressure[16]="?", visibility[16]="?";
    char windspeed_kmph[16]="?", windspeed_mph[16]="?";
    char winddir[16]="?";
    char precip_mm[16]="?", precip_in[16]="?";
    char obs_time[64]="?";
    char weather_code_str[16]="0";
    char desc[MAX_FIELD]="?";

    /* current_condition block */
    const char *cc = strstr(buf, "\"current_condition\"");
    if (!cc) { fputs("Could not parse response.\n", stderr); free(buf); return 1; }

    json_str_from(cc, "temp_C",         temp_c,         sizeof(temp_c));
    json_str_from(cc, "temp_F",         temp_f,         sizeof(temp_f));
    json_str_from(cc, "FeelsLikeC",     feels_c,        sizeof(feels_c));
    json_str_from(cc, "FeelsLikeF",     feels_f,        sizeof(feels_f));
    json_str_from(cc, "humidity",       humidity,       sizeof(humidity));
    json_str_from(cc, "cloudcover",     cloudcover,     sizeof(cloudcover));
    json_str_from(cc, "pressure",       pressure,       sizeof(pressure));
    json_str_from(cc, "visibility",     visibility,     sizeof(visibility));
    json_str_from(cc, "windspeedKmph",  windspeed_kmph, sizeof(windspeed_kmph));
    json_str_from(cc, "windspeedMiles", windspeed_mph,  sizeof(windspeed_mph));
    json_str_from(cc, "winddir16Point", winddir,        sizeof(winddir));
    json_str_from(cc, "precipMM",       precip_mm,      sizeof(precip_mm));
    json_str_from(cc, "precipInches",   precip_in,      sizeof(precip_in));
    json_str_from(cc, "localObsDateTime", obs_time,     sizeof(obs_time));
    json_str_from(cc, "weatherCode",    weather_code_str, sizeof(weather_code_str));

    /* weatherDesc value (nested: "weatherDesc": [{"value": "..."}] ) */
    const char *wdesc = strstr(cc, "\"weatherDesc\"");
    if (wdesc) json_str_from(wdesc, "value", desc, sizeof(desc));

    int wcode = atoi(weather_code_str);
    if (strcmp(desc, "?") == 0 || desc[0] == '\0')
        snprintf(desc, sizeof(desc), "%s", weather_code_desc(wcode));

    /* nearest_area */
    char area_name[MAX_FIELD]="?", region[MAX_FIELD]="?", country[MAX_FIELD]="?";
    const char *na = strstr(buf, "\"nearest_area\"");
    if (na) {
        const char *an = strstr(na, "\"areaName\"");
        if (an) json_str_from(an, "value", area_name, sizeof(area_name));
        const char *rg = strstr(na, "\"region\"");
        if (rg) json_str_from(rg, "value", region, sizeof(region));
        const char *ct = strstr(na, "\"country\"");
        if (ct) json_str_from(ct, "value", country, sizeof(country));
    }

    /* ---- Display -------------------------------------------------- */
    printf("\n");
    printf("  Weather for zip: %s\n", zip);
    printf("  Location : %s, %s, %s\n", area_name, region, country);
    printf("  As of    : %s\n", obs_time);
    printf("\n");

    print_ascii_art(wcode);
    printf("\n");

    printf("  Condition    : %s\n", desc);
    if (use_celsius) {
        printf("  Temperature  : %s °C  (feels like %s °C)\n", temp_c, feels_c);
        printf("  Wind         : %s km/h %s\n", windspeed_kmph, winddir);
        printf("  Visibility   : %s km\n", visibility);
        printf("  Precipitation: %s mm\n", precip_mm);
    } else {
        printf("  Temperature  : %s °F  (feels like %s °F)\n", temp_f, feels_f);
        printf("  Wind         : %s mph %s\n", windspeed_mph, winddir);
        printf("  Visibility   : %s mi\n", visibility);
        printf("  Precipitation: %s in\n", precip_in);
    }
    printf("  Humidity     : %s%%\n", humidity);
    printf("  Cloud cover  : %s%%\n", cloudcover);
    printf("  Pressure     : %s hPa\n", pressure);
    printf("\n");
    printf("  Data: wttr.in (World Weather Online)  |  No API key required\n");
    printf("\n");

    free(buf);
    return 0;
}
