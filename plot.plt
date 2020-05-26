set term png size 1440,900
set output "graph.png"
#set logscale x
set logscale cb
#set logscale y
set title "Scaling and overhead properties of fork()"
set xrange [0:15677334]
set cbrange [0.01:1]
set yrange [0:92]
set cbtics 1.2
set xlabel "Number of Instructions per fuzz case (loop of hot loads, 2 inst/cycle)"
set ylabel "Number of cores"
set cblabel "Ratio of CPU time spent inside the fuzz case (1.0 means no overhead)"
set grid xtics ytics mxtics mytics
plot "bigone.txt" u 2:1:3 w image

