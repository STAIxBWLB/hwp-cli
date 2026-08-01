아래 표는 HTML fragment(계약: docs/design/18)로 작성한다. markdown(GFM) 표로는
표현할 수 없는 병합 셀을 colspan/rowspan으로 쓸 수 있다.

<table>
<tr><th>구분</th><th colspan="2">실적</th></tr>
<tr><td rowspan="2">상반기</td><td>1분기</td><td>12건</td></tr>
<tr><td>2분기</td><td>15건</td></tr>
<tr><td>합계</td><td colspan="2">27건</td></tr>
</table>
