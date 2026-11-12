// Development-only exporter. Run with JDK 21; no Java is used by rmeter at runtime.
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.time.*;
import java.time.format.*;
import java.time.temporal.*;
import java.time.zone.*;
import java.text.DateFormatSymbols;
import java.util.*;
import java.util.zip.GZIPOutputStream;

public class ExportDateData {
    static String json(Object value) {
        if (value == null) return "null";
        if (value instanceof Number || value instanceof Boolean) return value.toString();
        if (value instanceof Map<?,?> map) {
            var out = new StringJoiner(",", "{", "}");
            map.forEach((k,v) -> out.add(json(k.toString()) + ":" + json(v))); return out.toString();
        }
        if (value instanceof Iterable<?> list) { var out = new StringJoiner(",", "[", "]"); for (var v:list) out.add(json(v)); return out.toString(); }
        String text=value.toString(); var out=new StringBuilder("\"");
        for(int i=0;i<text.length();i++) { char c=text.charAt(i); switch(c) {
            case '\\': out.append("\\\\"); break; case '"': out.append("\\\""); break;
            default: if(c<32) out.append(String.format("\\u%04x",(int)c)); else out.append(c);
        }} return out.append('"').toString();
    }
    static Map<String,Object> locale(Locale locale) {
        var out=new TreeMap<String,Object>(); var patterns=new TreeMap<String,Object>();
        var base=LocalDateTime.of(2024,1,1,0,0);
        for(String p:List.of("MMM","MMMM","MMMMM","LLL","LLLL","LLLLL","EEE","EEEE","EEEEE","ccc","cccc","ccccc","QQQ","QQQQ","QQQQQ","qqq","qqqq","qqqqq","G","GGGG","GGGGG","a")) {
            var fmt=DateTimeFormatter.ofPattern(p,locale); var values=new ArrayList<String>();
            int count=switch(p.charAt(0)) { case 'M','L' -> 12; case 'E','c' -> 7; case 'Q','q' -> 4; default -> 2; };
            for(int i=0;i<count;i++) { var dt=switch(p.charAt(0)) {
                case 'M','L' -> base.withMonth(i+1); case 'E','c' -> base.plusDays(i);
                case 'Q','q' -> base.withMonth(i*3+1); case 'G' -> base.withYear(i==0?0:2024); default -> base.withHour(i*12);
            }; values.add(fmt.format(dt)); } patterns.put(p,values);
        }
        // Day-period names and exact minute ranges, including midnight/noon points.
        for(String p:List.of("B","BBBB","BBBBB")) {
            var fmt=DateTimeFormatter.ofPattern(p,locale); var ranges=new ArrayList<Object>();
            String last=""; int start=0;
            for(int i=0;i<=1440;i++) { String current=i==1440?"":fmt.format(base.plusMinutes(i));
                if(!current.equals(last)) { if(i>0) ranges.add(List.of(start,i,last)); start=i; last=current; }
            } out.put(p,ranges);
        }
        out.put("patterns",patterns); var week=WeekFields.of(locale);
        out.put("first",week.getFirstDayOfWeek().getValue()); out.put("minimal",week.getMinimalDaysInFirstWeek());
        out.put("gmt",DateTimeFormatter.ofPattern("OOOO",locale).format(OffsetTime.of(0,0,0,0,ZoneOffset.UTC)));
        var names=new TreeMap<String,Object>(); var parseNames=new TreeMap<String,String>();var parseFullNames=new TreeMap<String,String>();
        var parse=DateTimeFormatter.ofPattern("z",locale);
        var parseFull=DateTimeFormatter.ofPattern("zzzz",locale);
        for(String[] row:DateFormatSymbols.getInstance(locale).getZoneStrings()) {
            if(!ZoneId.getAvailableZoneIds().contains(row[0]) || row.length<7) continue;
            names.put(row[0],Arrays.asList(row).subList(1,7));
            for(int i=1;i<7;i++) if(!row[i].isEmpty()) {
                if(!parseNames.containsKey(row[i]))try { parseNames.put(row[i],parse.parse(row[i],TemporalQueries.zone()).getId()); } catch(Exception ignored) {}
                if(!parseFullNames.containsKey(row[i]))try { parseFullNames.put(row[i],parseFull.parse(row[i],TemporalQueries.zone()).getId()); } catch(Exception ignored) {}
            }
        }
        for(String zone:ZoneId.getAvailableZoneIds()) if(!names.containsKey(zone)) {
            var values=new ArrayList<String>();
            for(String p:List.of("zzzz","z","zzzz","z","vvvv","v")) values.add(DateTimeFormatter.ofPattern(p,locale).withZone(ZoneId.of(zone)).format(Instant.EPOCH));
            names.put(zone,values);
        }
        out.put("zones",names); out.put("zone_parse",parseNames);out.put("zone_parse_full",parseFullNames); return out;
    }
    static Map<String,Object> zones() throws Exception {
        var out=new TreeMap<String,Object>();
        for(String id:new TreeSet<>(ZoneId.getAvailableZoneIds())) {
            var rule=ZoneId.of(id).getRules(); var transitions=new ArrayList<Object>();
            for(var t:rule.getTransitions()) transitions.add(List.of(t.toEpochSecond(), t.getOffsetBefore().getTotalSeconds(),t.getOffsetAfter().getTotalSeconds(),rule.isDaylightSavings(t.getInstant())));
            var recurring=new ArrayList<Object>();
            for(var r:rule.getTransitionRules()) recurring.add(List.of(r.getMonth().getValue(),r.getDayOfMonthIndicator(),r.getDayOfWeek()==null?0:r.getDayOfWeek().getValue(),r.getLocalTime().toSecondOfDay(),r.isMidnightEndOfDay(),r.getTimeDefinition().name(),r.getStandardOffset().getTotalSeconds(),r.getOffsetBefore().getTotalSeconds(),r.getOffsetAfter().getTotalSeconds()));
            var sf=ZoneRules.class.getDeclaredField("standardTransitions");sf.setAccessible(true);
            var of=ZoneRules.class.getDeclaredField("standardOffsets");of.setAccessible(true);
            long[] changes=(long[])sf.get(rule);ZoneOffset[] offsets=(ZoneOffset[])of.get(rule);
            var standard=new ArrayList<Object>();
            standard.add(List.of(Long.MIN_VALUE,offsets[0].getTotalSeconds()));
            for(int i=0;i<changes.length;i++)standard.add(List.of(changes[i],offsets[i+1].getTotalSeconds()));
            var method=Class.forName("sun.util.locale.provider.TimeZoneNameUtility").getMethod("explicitDstOffset",String.class);
            String explicit=(String)method.invoke(null,id);
            out.put(id,Map.of("initial", rule.getOffset(Instant.MIN).getTotalSeconds(),"transitions",transitions,"rules",recurring,"standard",standard,"explicit_dst",explicit==null?Integer.MIN_VALUE:ZoneOffset.of(explicit).getTotalSeconds()));
        } return out;
    }
    static byte[] gzip(Object value) throws Exception { var bytes=new ByteArrayOutputStream(); try(var zip=new GZIPOutputStream(bytes)){zip.write(json(value).getBytes(StandardCharsets.UTF_8));} return bytes.toByteArray(); }
    public static void main(String[] args) throws Exception {
        if(Runtime.version().feature()!=21) throw new IllegalStateException("Use JDK 21");
        Path dir=Path.of(args[0]); Files.createDirectories(dir); var index=new TreeMap<String,Object>();
        try(var bundle=Files.newOutputStream(dir.resolve("locales.bin"))) {
            long offset=0; int n=0; var locales=new TreeMap<String,Locale>();
            for(var l:Locale.getAvailableLocales()) locales.put(l.toLanguageTag(),l);
            for(var entry:locales.entrySet()) { byte[] bytes=gzip(locale(entry.getValue())); bundle.write(bytes); index.put(entry.getKey(),List.of(offset,bytes.length)); offset+=bytes.length; if(++n%50==0) System.out.println("Locales "+n+"/"+locales.size()); }
        }
        Files.writeString(dir.resolve("locales.json"),json(index),StandardCharsets.UTF_8);
        Files.write(dir.resolve("zones.json.gz"),gzip(zones()));
        var titlecase=new TreeMap<String,String>();
        for(int cp=0;cp<=Character.MAX_CODE_POINT;cp++) {
            if(cp>=0xd800&&cp<=0xdfff)continue;
            int title=Character.toTitleCase(cp);
            if(title!=cp)titlecase.put(Integer.toString(cp),new String(Character.toChars(title)));
        }
        Files.write(dir.resolve("titlecase.json.gz"),gzip(titlecase));
        Files.writeString(dir.resolve("version.json"),json(Map.of("java",System.getProperty("java.runtime.version"),"vendor",System.getProperty("java.vendor"),"tzdb",ZoneRulesProvider.getVersions("UTC").lastKey(),"locale_provider","CLDR,COMPAT")),StandardCharsets.UTF_8);
    }
}
