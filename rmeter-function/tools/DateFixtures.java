// Generate checked-in differential cases using the same JDK 21 as date data.
import java.nio.file.*;
import java.nio.charset.StandardCharsets;
import java.io.*;
import java.time.*;
import java.time.format.*;
import java.util.*;

public class DateFixtures {
    static BufferedWriter out; static int count;
    static void test(String input,String source,String target,String locale,String zone) throws Exception {
        var row=new TreeMap<String,Object>();row.put("input",input);row.put("source",source);row.put("target",target);row.put("locale",locale);row.put("zone",zone);
        try {var l=Locale.forLanguageTag(locale);var z=ZoneId.of(zone);var fmt=DateTimeFormatter.ofPattern(target,l).withZone(z);
            String value=source.isEmpty()?fmt.format(Instant.ofEpochMilli(Long.parseLong(input))):fmt.format(DateTimeFormatter.ofPattern(source,l).withZone(z).parse(input));row.put("expected",value);
        }catch(Exception e){row.put("expected",null);row.put("error",e.toString());}out.write(ExportDateData.json(row));out.newLine();count++;
    }
    public static void main(String[] args) throws Exception {
        if(Runtime.version().feature()!=21)throw new IllegalStateException("JDK 21 required");
        out=new BufferedWriter(new OutputStreamWriter(new java.util.zip.GZIPOutputStream(Files.newOutputStream(Path.of(args[0]))),StandardCharsets.UTF_8));
        String[] locales={"en-US","en-GB","zh-CN","zh-TW","ja-JP","fr-FR","de-DE","ru-RU","ar-EG","hi-IN","pl-PL","th-TH"};
        String[] zones={"UTC","Asia/Shanghai","Europe/Paris","America/New_York","Australia/Lord_Howe","+05:45"};
        var patterns=new ArrayList<String>();
        for(char c:"GuyYDMLdQqYwWEecFaBhKkHmsSAnNVvzOXxZg".toCharArray())for(int n=1;n<=6;n++)patterns.add(String.valueOf(c).repeat(n));
        patterns.addAll(List.of("SSSSSSSSS","yyyyMMddHHmmss","uuuu-MM-dd'T'HH:mm:ss[.SSS]XXX","ppH","'it''s' yyyy","[qqqq] [VV] [HH:mm]","uuuuuuuuuuuuuuuuuuu","[","''","[yyyy","yyyy]"));
        for(String locale:locales)for(String zone:zones)for(String millis:List.of("0","-1","1709168523123","1609459200000","1710055800123","1730611800123"))for(String p:patterns)test(millis,"",p,locale,zone);
        String[][] parses={
            {"2024-02-30","yyyy-MM-dd","uuuu-MM-dd EEEE"},{"2023-02-29","yyyy-MM-dd","yyyyMMdd"},
            {"2024-13-01","yyyy-MM-dd","yyyy-MM-dd"},{"2024-02-32","yyyy-MM-dd","yyyy-MM-dd"},
            {"2024-12-31 24:00","yyyy-MM-dd HH:mm","yyyy-MM-dd HH:mm:ss"},{"24:01","HH:mm","HH:mm"},
            {"2024-02-29","yyyy-MM-dd['T'HH:mm[:ss]]","yyyy/MM/dd[ HH:mm:ss]"},
            {"20240229123456","yyyyMMddHHmmss","yyyy-MM-dd HH:mm:ss"},
            {"2000-01-01T00:00:00.123456789+05:45","uuuu-MM-dd'T'HH:mm:ss.SSSSSSSSSXXX","uuuu-MM-dd HH:mm:ss.SSSSSSSSS XXX VV"},
            {"2024-03-10 02:30 America/New_York","yyyy-MM-dd HH:mm VV","yyyy-MM-dd HH:mm XXX VV"},
            {"2024-11-03 01:30 America/New_York","yyyy-MM-dd HH:mm VV","yyyy-MM-dd HH:mm XXX VV"},
            {"-999999999-01-01","uuuu-MM-dd","G uuuu-MM-dd D EEEE YYYY-ww-e"},
            {"+999999999-12-31","uuuu-MM-dd","G uuuu-MM-dd D EEEE YYYY-ww-e"},
            {"0000-01-01","uuuu-MM-dd","G yyyy uuuu"},{"1 BC","y G","uuuu G"},
            {"2020-53-7","YYYY-ww-e","yyyy-MM-dd"},{"2020-01-1","YYYY-ww-e","yyyy-MM-dd"},
            {"2024-060","yyyy-DDD","yyyy-MM-dd"},{"60369","g","yyyy-MM-dd"},
            {"2024-02-5-1","yyyy-MM-W-e","yyyy-MM-dd"},{"2024 Q1","yyyy QQQ","yyyy QQQQ"},
            {"12:30 PM","hh:mm a","HH:mm"},{"00:30","kk:mm","HH:mm"},
            {"43200123","A","HH:mm:ss.SSS"},{"43200123456789","N","HH:mm:ss.SSSSSSSSS"},
            {" 3","ppH","HH"},{"2024-02-29 Thu","yyyy-MM-dd EEE","yyyy-MM-dd"},
            {"2024-02-29 Fri","yyyy-MM-dd EEE","yyyy-MM-dd"},{"+2024","yyyy","yyyy"},
            {"99","yy","yyyy"},{"-00","uu","uuuu"},{"2024","yyyy","yyyy-MM"},
            {"2024-01-01 08:00 +0800","yyyy-MM-dd HH:mm Z","yyyy-MM-dd HH:mm XXX"},
            {"2024-01-01 08:00 GMT+8","yyyy-MM-dd HH:mm O","yyyy-MM-dd HH:mm XXX"},
            {"2024-01-01 08:00 CST","yyyy-MM-dd HH:mm z","yyyy-MM-dd HH:mm XXX VV"}
        };
        for(String locale:locales)for(String zone:zones)for(String[] p:parses)test(p[0],p[1],p[2],locale,zone);
        // Text parsing and numeric adjacency, generated independently by Java.
        for(String locale:locales){var l=Locale.forLanguageTag(locale);var dt=LocalDateTime.of(2024,7,16,13,25,47,123456789);
            for(String p:List.of("d MMM uuuu HH:mm","d MMMM uuuu hh:mm a","yyyyMMddHHmmssSSS","yyyy-DDD HH:mm:ss","YYYY-ww-e HH:mm","yyyy-MM-W-e HH:mm","yyyy-MM-dd h:mm B","yyyy-MM-dd HH:mm BBBBB","yyyy-MM-dd 'test' HH:mm:ss.SSSSSSSSS")){
                String s=DateTimeFormatter.ofPattern(p,l).format(dt);test(s,p,"uuuu-MM-dd HH:mm:ss.SSSSSSSSS",locale,"UTC");
            }
        }
        // Every locale and every zone, plus parsing each field independently.
        for(Locale locale:Locale.getAvailableLocales()) {
            String tag=locale.toLanguageTag();
            test("1709168523123","","G MMMM LLLL EEEE cccc QQQQ qqqq a BBBB YYYY-ww-e W OOOO zzzz vvvv",tag,"Europe/Paris");
            for(String p:List.of("MMM","MMMM","MMMMM","LLL","LLLL","LLLLL","EEE","EEEE","EEEEE","ccc","cccc","ccccc","QQQ","QQQQ","QQQQQ","qqq","qqqq","qqqqq","G","GGGG","GGGGG","a","B","BBBB","BBBBB")) {
                String text=DateTimeFormatter.ofPattern(p,locale).format(LocalDateTime.of(2024,7,16,13,25));
                test(text,p,p,tag,"UTC");
            }
        }
        for(String zone:ZoneId.getAvailableZoneIds()) for(String millis:List.of("-2208988800000","0","1710055800123","4102444800000"))test(millis,"","uuuu-MM-dd HH:mm:ss XXX zzzz vvvv","en-US",zone);
        for(String p:patterns) {
            try {String text=DateTimeFormatter.ofPattern(p,Locale.US).format(LocalDateTime.of(2024,7,16,13,25,47,123456789));test(text,p,p,"en-US","UTC");} catch(Exception ignored) {}
        }
        String[][] more={
            {"30","m","mm"},{"30","s","ss"},{"123","SSS","n"},{"12","h","h"},{"1","a","a"},
            {"2024-02-29 061","yyyy-MM-dd DDD","yyyy-MM-dd"},
            {"2024-01-01 2025","yyyy-MM-dd YYYY","yyyy-MM-dd"},
            {"2024-01-01 02","yyyy-MM-dd ww","yyyy-MM-dd"},
            {"2024-01-01 2","yyyy-MM-dd F","yyyy-MM-dd"},
            {"2024-01-01 00:00:01","yyyy-MM-dd HH:mm:ss","B BBBB BBBBB"},
            {"2024-01-01 12:00:01","yyyy-MM-dd HH:mm:ss","B BBBB BBBBB"},
            {"at night","B","HH:mm"},{"12 at night","h B","HH:mm"},
            {"2024-01-01 10:30:01 123","yyyy-MM-dd HH:mm:ss n","A N"},
            {"2024-01-01 10:30:01 123456789","yyyy-MM-dd HH:mm:ss n","A N"},
            {"+18:00","XXX","XXXXX"},{"-00:00","XXX","XXXXX"},{"+18:01","XXX","XXXXX"},
            {"+0800","XXX","XXX"},{"+08:00:12","XXXXX","XXXXX"},
            {"+01::00","XXXXX","XXXXX"},{"2024","yyyy'xx'","yyyy"},
            {"99","YYYY","YYYY"},{"+001","uuu","uuu"},{"-0001","uuuu","uuuu"}
        };
        for(String locale:locales)for(String[] p:more)test(p[0],p[1],p[2],locale,"UTC");
        for(String p:List.of("u","y","Y","D","d","M","Q","w","W","F","e","H","h","K","k","m","s","n","A","N","g")) {
            for(String value:List.of("-1","0","1","12","24","31","59","60","99","365","366","999999999","1000000000","9223372036854775807","-9223372036854775808"))test(value,p,p,"en-US","UTC");
        }
        for(String p:List.of("H s","H n","m s","s n","H m n","K a","h a","a","n","G","E","e","YYYY-MM-dd")) {
            try {String value=DateTimeFormatter.ofPattern(p,Locale.US).format(LocalDateTime.of(2024,7,16,13,25,47,123456789));
                for(String target:List.of("HH:mm:ss.SSSSSSSSS","H m s n","uuuu-MM-dd","u y G E e"))test(value,p,target,"en-US","UTC");
            }catch(Exception ignored){}
        }
        for(String p:List.of("YYYYwwe","yyyyMMddHSSS","yyMMdd","yyyyDDD","[pHH]","[ppHH]","[pppMMMM]","[yyyy][HH:mm]")) {
            var dt=LocalDateTime.of(2024,7,16,13,25,47,123456789);
            test("1721136347123","",p,"en-US","UTC");
            try{String value=DateTimeFormatter.ofPattern(p,Locale.US).format(dt);test(value,p,"uuuu-MM-dd[ HH:mm:ss.SSS]","en-US","UTC");}catch(Exception ignored){}
        }
        for(String locale:locales)for(String zone:zones)for(String suffix:List.of("z","zzzz","v","vvvv"))for(int month:List.of(1,7)){
            String p="uuuu-MM-dd HH:mm:ss "+suffix;
            String value=DateTimeFormatter.ofPattern(p,Locale.forLanguageTag(locale)).format(ZonedDateTime.of(2024,month,16,13,25,47,0,ZoneId.of(zone)));
            test(value,p,"uuuu-MM-dd HH:mm:ss XXX VV",locale,"UTC");
        }
        for(String source:List.of("M","Q"))for(String value:List.of("-9223372036854775808","-1","0","13","9223372036854775807"))for(String target:List.of("MMM","QQQQ"))test(value,source,target,"en-US","UTC");
        Files.writeString(Path.of(args[0]).resolveSibling("version.json"),ExportDateData.json(Map.of("java",System.getProperty("java.runtime.version"),"vendor",System.getProperty("java.vendor"),"tzdb",java.time.zone.ZoneRulesProvider.getVersions("UTC").lastKey(),"cases",count)),StandardCharsets.UTF_8);
        out.close();System.out.println("Generated "+count+" JDK 21 cases");
    }
}
