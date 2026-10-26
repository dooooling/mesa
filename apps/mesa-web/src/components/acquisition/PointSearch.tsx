// 点位搜索 + 筛选（通用组件，§2.8）。
//
// 只认关键字与筛选项，不认识任何驱动/协议：筛选项由页面层**按实际数据**生成
// （没有问题时不会出现「仅看问题」入口）。
import { CloseCircleFilled, SearchOutlined } from "@ant-design/icons";
import { Input, Select } from "antd";
import type { PointFilter, PointFilterOption } from "../../acquisitionRowView";

export function PointSearch({
  query,
  placeholder,
  onChange,
  filters,
  filter,
  onFilterChange,
}: {
  query: string;
  placeholder: string;
  onChange: (query: string) => void;
  filters: PointFilterOption[];
  filter: PointFilter;
  onFilterChange: (filter: PointFilter) => void;
}) {
  return (
    <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
      <Input
        data-testid="point-search"
        aria-label="搜索已配置点位"
        allowClear={{ clearIcon: <CloseCircleFilled /> }}
        prefix={<SearchOutlined style={{ color: "#8c8c8c" }} />}
        placeholder={placeholder}
        value={query}
        onChange={(e) => onChange(e.target.value)}
        style={{ width: 360 }}
      />
      {/* 只有「全部」时不渲染下拉：没有可筛的维度就不占位置。 */}
      {filters.length > 1 ? (
        <Select
          data-testid="point-filter"
          aria-label="筛选点位"
          size="small"
          style={{ width: 150 }}
          value={filter}
          onChange={(value) => onFilterChange(value as PointFilter)}
          options={filters.map((item) => ({ value: item.value, label: item.label }))}
        />
      ) : null}
    </div>
  );
}
