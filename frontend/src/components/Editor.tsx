import React, { useRef, useEffect, useState } from 'react';

interface EditorProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  language?: string;
  readOnly?: boolean;
}

const Editor: React.FC<EditorProps> = ({
  value,
  onChange,
  placeholder = 'Write your code here...',
  language = 'javascript',
  readOnly = false,
}) => {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const lineNumbersRef = useRef<HTMLDivElement>(null);
  const [lineCount, setLineCount] = useState(1);

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    const handleScroll = () => {
      if (lineNumbersRef.current) {
        lineNumbersRef.current.scrollTop = textarea.scrollTop;
      }
    };

    textarea.addEventListener('scroll', handleScroll);
    return () => textarea.removeEventListener('scroll', handleScroll);
  }, []);

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newValue = e.target.value;
    onChange(newValue);
    updateLineCount(newValue);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    // Tab support
    if (e.key === 'Tab') {
      e.preventDefault();
      const start = textarea.selectionStart;
      const end = textarea.selectionEnd;
      const newValue = value.substring(0, start) + '\t' + value.substring(end);
      onChange(newValue);

      setTimeout(() => {
        textarea.selectionStart = textarea.selectionEnd = start + 1;
      }, 0);
    }
  };

  const updateLineCount = (text: string) => {
    const lines = text.split('\n').length;
    setLineCount(lines);
  };

  useEffect(() => {
    updateLineCount(value);
  }, [value]);

  return (
    <div className="flex h-full w-full flex-col overflow-hidden rounded-2xl border border-zinc-300/60 bg-white dark:bg-[#1e1e1e] dark:border-white/10">
      <div className="flex w-full h-full relative font-mono text-sm leading-relaxed text-gray-800 dark:text-[#e0e0e0]">
        <div
          ref={lineNumbersRef}
          className="flex min-w-12.5 select-none flex-col overflow-hidden border-r border-zinc-300/50 bg-gray-100 dark:bg-[#252526] py-3 pl-0 pr-4 text-right text-gray-500 dark:text-[#858585] dark:border-white/10"
        >
          {Array.from({ length: lineCount }, (_, i) => (
            <div key={i + 1} className="h-[1.6em] text-sm leading-relaxed">
              {i + 1}
            </div>
          ))}
        </div>
        <textarea
          ref={textareaRef}
          className="flex-1 p-3 bg-transparent border-0 outline-none font-mono text-sm leading-relaxed resize-none overflow-y-scroll overflow-x-auto whitespace-pre break-normal appearance-none text-gray-800 placeholder-gray-400 dark:text-[#e0e0e0] dark:placeholder-[#555] disabled:opacity-60 disabled:cursor-not-allowed focus:outline-none"
          value={value}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          data-language={language}
          readOnly={readOnly}
          spellCheck="false"
        />
      </div>
    </div>
  );
};

export default Editor;
